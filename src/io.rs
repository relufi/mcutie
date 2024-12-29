use core::ops::Deref;

pub(crate) use atomic16::assign_pid;
use embassy_futures::select::{select, select3, Either};
use embassy_net::{dns::DnsQueryType, tcp::{TcpReader, TcpSocket, TcpWriter}, IpEndpoint, Stack};
use embassy_sync::pubsub::WaitResult;
use embassy_time::{Duration, Timer};
use embedded_io_async::Write;
use mqttrs::{
    decode_slice, Connect, ConnectReturnCode, LastWill, Packet, Pid, Protocol, Publish, QoS, QosPid,
};

use crate::{fmt::Debug2Format, ControlMessage, ControlSubscriber, Error, IpDn, McutieReceiver, McutieSender, MqttMessage, Payload, Publishable, Topic, TopicString, CONFIRMATION_TIMEOUT, DEFAULT_BACKOFF, RESET_BACKOFF};


pub(crate) async fn subscribe<'a>(sender: &'a McutieSender) -> ControlSubscriber<'a> {
    loop {
        if let Ok(sub) = sender.control_channel.subscriber() {
            return sub;
        }

        Timer::after_millis(50).await;
    }
}

mod atomic16 {

    use mqttrs::Pid;
    use crate::McutieSender;

    pub(crate) async fn assign_pid(sender: &'_ McutieSender) -> Pid {
        let new_val = sender.count.get() + 1;
        sender.count.set(new_val);
        Pid::new() + new_val
    }
}

pub(crate) async fn send_packet(sender: &'_ McutieSender,packet: Packet<'_>) -> Result<(), Error> {
    let mut writer = sender.sender.writer().await;
    match writer.write().encode_packet(&packet) {
        Ok(()) => {
            trace!("Pushing new packet for broker");
            Ok(())
        }
        Err(_) => {
            error!("Failed to send packet");
            Err(Error::PacketError)
        }
    }
}

pub(crate) async fn wait_for_publish(
    mut subscriber: ControlSubscriber<'_>,
    expected_pid: Pid,
) -> Result<(), Error> {
    match select(
        async {
            loop {
                match subscriber.next_message().await {
                    WaitResult::Lagged(_) => {
                        // Maybe we missed the message?
                    }
                    WaitResult::Message(ControlMessage::Published(published_pid)) => {
                        if published_pid == expected_pid {
                            return Ok(());
                        }
                    }
                    _ => {}
                }
            }
        },
        Timer::after_millis(CONFIRMATION_TIMEOUT),
    )
    .await
    {
        Either::First(r) => r,
        Either::Second(_) => Err(Error::TimedOut),
    }
}

pub(crate) async fn publish(
    sender: &'_ mut McutieSender,
    topic_name: &str,
    payload: &[u8],
    qos: QoS,
    retain: bool,
) -> Result<(), Error> {
    let subscriber = subscribe(sender).await;

    let (qospid, pid) = match qos {
        QoS::AtMostOnce => (QosPid::AtMostOnce, None),
        QoS::AtLeastOnce => {
            let pid = assign_pid(sender).await;
            (QosPid::AtLeastOnce(pid), Some(pid))
        }
        QoS::ExactlyOnce => {
            let pid = assign_pid(sender).await;
            (QosPid::ExactlyOnce(pid), Some(pid))
        }
    };

    let packet = Packet::Publish(Publish {
        dup: false,
        qospid,
        retain,
        topic_name,
        payload,
    });

    send_packet(sender,packet).await?;

    if let Some(expected_pid) = pid {
        wait_for_publish(subscriber, expected_pid).await
    } else {
        Ok(())
    }
}

fn packet_size(buffer: &[u8]) -> Option<usize> {
    let mut pos = 1;
    let mut multiplier = 1;
    let mut value = 0;

    while pos < buffer.len() {
        value += (buffer[pos] & 127) as usize * multiplier;
        multiplier *= 128;

        if (buffer[pos] & 128) == 0 {
            return Some(value + pos + 1);
        }

        pos += 1;
        if pos == 5 {
            return Some(0);
        }
    }

    None
}

/// The MQTT task that must be run in order for the stack to operate.
pub struct McutieTask<'t,'a, T, L, const S: usize>
where
    T: Deref<Target = str> + 't,
    L: Publishable + 't,
{
    pub(crate) network: Stack<'t>,
    pub(crate) broker: IpDn<'t>,
    pub(crate) tcp_time_out: u64,
    pub(crate) tcp_keep_alive: u64,
    pub(crate) mqtt_ping_secs: u16,
    pub(crate) mqtt_keep_alive: u16,
    pub(crate) client_id: &'t str,
    pub(crate) last_will: Option<L>,
    pub(crate) username: Option<&'t str>,
    pub(crate) password: Option<&'t str>,
    pub(crate) subscriptions: [Topic<T>; S],
    pub receive: &'a McutieReceiver,
    pub sender: &'a McutieSender,
}

impl<'t,'a, T, L, const S: usize> McutieTask<'t,'a, T, L, S>
where
    T: Deref<Target = str> + 't,
    L: Publishable + 't,
{
    async fn ha_handle_update(&self, _topic: &Topic<TopicString>, _payload: &Payload) -> bool {
        false
    }

    pub async fn send_packet(&self,packet: Packet<'_>) -> Result<(), Error> {
        send_packet(self.sender,packet).await
    }


    async fn recv_loop(&self, mut reader: TcpReader<'_>) -> Result<(), Error> {
        let mut buffer = [0_u8; 4096];
        let mut cursor: usize = 0;

        let controller = self.sender.control_channel.immediate_publisher();

        loop {
            match reader.read(&mut buffer[cursor..]).await {
                Ok(0) => {
                    error!("Receive socket closed");
                    return Ok(());
                }
                Ok(len) => {
                    cursor += len;
                }
                Err(_) => {
                    error!("I/O failure reading packet");
                    return Err(Error::IOError);
                }
            }

            let mut start_pos = 0;
            loop {
                let packet_length = match packet_size(&buffer[start_pos..cursor]) {
                    Some(0) => {
                        error!("Invalid MQTT packet");
                        return Err(Error::PacketError);
                    }
                    Some(len) => len,
                    None => {
                        // None is returned when there is not yet enough data to decode a packet.
                        if start_pos != 0 {
                            // Adjust the buffer to reclaim any unused data
                            buffer.copy_within(start_pos..cursor, 0);
                            cursor -= start_pos;
                        }
                        break;
                    }
                };

                let packet = match decode_slice(&buffer[start_pos..(start_pos + packet_length)]) {
                    Ok(Some(p)) => p,
                    Ok(None) => {
                        error!("Packet length calculation failed.");
                        return Err(Error::PacketError);
                    }
                    Err(_) => {
                        error!("Invalid MQTT packet");
                        return Err(Error::PacketError);
                    }
                };

                trace!(
                    "Received packet from broker: {:?}",
                    Debug2Format(&packet.get_type())
                );

                match packet {
                    Packet::Connack(connack) => match connack.code {
                        ConnectReturnCode::Accepted => {
                            for topic in &self.subscriptions {
                                let _ = topic.subscribe(self.sender,false).await;
                            }

                            self.receive.send(MqttMessage::Connected).await;
                        }
                        _ => {
                            error!("Connection request to broker was not accepted");
                            return Err(Error::IOError);
                        }
                    },
                    Packet::Pingresp => {}

                    Packet::Publish(publish) => {
                        match (
                            Topic::from_str(publish.topic_name),
                            Payload::from(publish.payload),
                        ) {
                            (Ok(topic), Ok(payload)) => {
                                if !self.ha_handle_update(&topic, &payload).await {
                                    self.receive
                                        .send(MqttMessage::Publish(topic, payload))
                                        .await;
                                }
                            }
                            _ => {
                                error!("Unable to process publish data as it was too large");
                            }
                        }

                        match publish.qospid {
                            mqttrs::QosPid::AtMostOnce => {}
                            mqttrs::QosPid::AtLeastOnce(pid) => {
                                self.send_packet(Packet::Puback(pid)).await?;
                            }
                            mqttrs::QosPid::ExactlyOnce(pid) => {
                                self.send_packet(Packet::Pubrec(pid)).await?;
                            }
                        }
                    }
                    Packet::Puback(pid) => {
                        controller.publish_immediate(ControlMessage::Published(pid));
                    }
                    Packet::Pubrec(pid) => {
                        controller.publish_immediate(ControlMessage::Published(pid));
                        self.send_packet(Packet::Pubrel(pid)).await?;
                    }
                    Packet::Pubrel(pid) => self.send_packet(Packet::Pubrel(pid)).await?,
                    Packet::Pubcomp(_) => {}

                    Packet::Suback(suback) => {
                        if let Some(return_code) = suback.return_codes.first() {
                            controller.publish_immediate(ControlMessage::Subscribed(
                                suback.pid,
                                *return_code,
                            ));
                        } else {
                            warn!("Unexpected suback with no return codes");
                        }
                    }
                    Packet::Unsuback(pid) => {
                        controller.publish_immediate(ControlMessage::Unsubscribed(pid));
                    }

                    Packet::Connect(_)
                    | Packet::Subscribe(_)
                    | Packet::Pingreq
                    | Packet::Unsubscribe(_)
                    | Packet::Disconnect => {
                        debug!(
                            "Unexpected packet from broker: {:?}",
                            Debug2Format(&packet.get_type())
                        );
                    }
                }

                start_pos += packet_length;
                if start_pos == cursor {
                    cursor = 0;
                    break;
                }
            }
        }
    }

    async fn write_loop(&self, mut writer: TcpWriter<'_>) {
        let mut buffer = Payload::new();

        let mut last_will_topic = TopicString::new();
        let mut last_will_payload = Payload::new();

        let last_will = self.last_will.as_ref().and_then(|p| {
            if p.write_topic(&mut last_will_topic).is_ok()
                && p.write_payload(&mut last_will_payload).is_ok()
            {
                Some(LastWill {
                    topic: &last_will_topic,
                    message: &last_will_payload,
                    qos: p.qos(),
                    retain: p.retain(),
                })
            } else {
                None
            }
        });

        // Send our connection request.
        if buffer
            .encode_packet(&Packet::Connect(Connect {
                protocol: Protocol::MQTT311,
                keep_alive: self.mqtt_keep_alive,
                client_id: self.client_id,
                clean_session: true,
                last_will,
                username: self.username,
                password: self.password.map(|s| s.as_bytes()),
            }))
            .is_err()
        {
            error!("Failed to encode connection packet");
            return;
        }

        if let Err(e) = writer.write_all(&buffer).await {
            error!("Failed to send connection packet: {:?}", e);
            return;
        }

        let reader =  self.sender.sender.reader();
        loop {
            trace!("Writer waiting for data");
            let buffer = reader.receive().await;

            trace!("Writer sending data");
            if let Err(e) = writer.write_all(buffer.read()).await {
                error!("Failed to send data: {:?}", e);
                return;
            }
        }
    }

    /// Runs the MQTT stack. The future returns from this must be awaited for everything to work.
    pub async fn run(&self) {
        let mut timeout: Option<u64> = None;

        let mut rx_buffer = [0; 4096];
        let mut tx_buffer = [0; 4096];

        loop {
            if let Some(millis) = timeout.replace(DEFAULT_BACKOFF) {
                Timer::after_millis(millis).await;
            }

            if !self.network.is_config_up() {
                trace!("Waiting for network to configure.");
                let _ = embassy_time::with_timeout(Duration::from_secs(10),self.network.wait_config_up()).await;
                trace!("Network configured.");
            }

            let ip_addrs = self.network.dns_query(self.broker.hostname, DnsQueryType::A).await;
            let ip = ip_addrs.ok().and_then(|mut ip_addrs| ip_addrs.pop())
                .unwrap_or(self.broker.back_ip);
            let ip = IpEndpoint::new(ip, self.broker.port);
            // trace!("Connecting to {}", ip);

            let mut socket = TcpSocket::new(self.network, &mut rx_buffer, &mut tx_buffer);
            if self.tcp_time_out != 0 {
                socket.set_timeout(Some(embassy_time::Duration::from_secs(self.tcp_time_out)));
            }
            if self.tcp_keep_alive != 0 {
                socket.set_keep_alive(Some(embassy_time::Duration::from_secs(self.tcp_keep_alive)));
            }
            if let Err(_e) = socket.connect(ip).await {
                // error!("Failed to connect to {}:1883: {:?}", ip, e);
                continue;
            }

            info!("Connected to {}", self.broker.hostname);
            timeout = Some(RESET_BACKOFF);

            let (reader, writer) = socket.split();
            let recv_loop = self.recv_loop(reader);
            let send_loop = self.write_loop(writer);

            let ping_loop = async {
                loop {
                    Timer::after_secs(self.mqtt_ping_secs as u64).await;

                    let _ = send_packet(self.sender,Packet::Pingreq).await;
                }
            };

            select3(send_loop, ping_loop, recv_loop).await;

            socket.close();

            self.receive.send(MqttMessage::Disconnected).await;
        }
    }
}
