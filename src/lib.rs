#![no_std]
#![doc = include_str!("../README.md")]
#![deny(unreachable_pub)]
// #![warn(missing_docs)]
#![cfg_attr(docsrs, feature(doc_auto_cfg))]

use core::{ops::Deref, str};
use core::cell::Cell;
pub use buffer::Buffer;
use embassy_net::{IpAddress, Stack};
use embassy_sync::channel::Channel;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::pubsub::{PubSubChannel, Subscriber};
use heapless::String;
pub use io::McutieTask;
pub use mqttrs::QoS;
use mqttrs::{Pid, SubscribeReturnCodes};
pub use publish::*;
pub use topic::Topic;
use crate::pipe::ConnectedPipe;

// This must come first so the macros are visible
pub(crate) mod fmt;

mod buffer;
mod io;
mod pipe;
mod publish;
mod topic;

// This really needs to match that used by mqttrs.
const TOPIC_LENGTH: usize = 256;
const PAYLOAD_LENGTH: usize = 2048;

/// A fixed length stack allocated string. The length is fixed by the mqttrs crate.
pub type TopicString = String<TOPIC_LENGTH>;
/// A fixed length buffer of 2048 bytes.
pub type Payload = Buffer<PAYLOAD_LENGTH>;

// By default in the event of an error connecting to the broker we will wait for 5s.
const DEFAULT_BACKOFF: u64 = 5000;
// If the connection dropped then re-connect more quickly.
const RESET_BACKOFF: u64 = 200;
// How long to wait for the broker to confirm actions.
// const CONFIRMATION_TIMEOUT: u64 = 2000;


/// Various errors
#[derive(Debug)]
pub enum Error {
    /// An IO error occured.
    IOError,
    /// The operation timed out.
    TimedOut,
    /// An attempt was made to encode something too large.
    TooLarge,
    /// A packet or payload could not be decoded or encoded.
    PacketError,
}

#[allow(clippy::large_enum_variant)]
/// A message from the MQTT broker.
pub enum MqttMessage {
    /// The broker has been connected to successfully. Generally in response to this message a
    /// device should subscribe to topics of interest and send out any device state.
    Connected,
    /// New data received from the broker.
    Publish(Topic<TopicString>, Payload),
    /// The connection to the broker has been dropped.
    Disconnected,
    /// Home Assistant has come online and you should send any discovery messages.
    #[cfg(feature = "homeassistant")]
    HomeAssistantOnline,
}

#[derive(Clone)]
enum ControlMessage {
    Published(Pid),
    Subscribed(Pid, SubscribeReturnCodes),
    Unsubscribed(Pid),
}
type ControlSubscriber<'a> = Subscriber<'a, NoopRawMutex, ControlMessage, 2, 5, 0>;
pub struct McutieSender {
    sender: ConnectedPipe<NoopRawMutex,Payload>,
    count: Cell<Pid>,
    control_channel:  PubSubChannel<NoopRawMutex, ControlMessage, 2, 5, 0>,
    confirmation_timeout: u64
}

impl McutieSender {
    // 2000
    pub fn new(confirmation_timeout: u64) -> Self {
        Self {
            sender: ConnectedPipe::new(Payload::new()),
            count: Cell::new(Pid::new()),
            control_channel: PubSubChannel::new(),
            confirmation_timeout,
        }
    }

    pub fn assign_pid(&self) -> Pid {
        let new_val = self.count.get() + 1;
        self.count.set(new_val);
        new_val
    }

}

/// Receives messages from the broker.
pub type McutieReceiver = Channel<NoopRawMutex, MqttMessage, 10>;

/// ip
#[derive(Clone)]
pub struct IpDn<'t>{
    /// dns hostname
    pub hostname: &'t str,
    /// port
    pub port: u16,
    /// back ip
    pub back_ip: IpAddress,
}


/// A builder to configure the MQTT stack.
pub struct McutieBuilder<'t, T, L, const S: usize>
where
    T: Deref<Target = str> + 't,
    L: Publishable + 't,
{
    network: Stack<'t>,
    tcp_time_out: u64,
    tcp_keep_alive: u64,
    mqtt_ping_secs: u16,
    mqtt_keep_alive: u16,
    device_id: &'t str,
    broker: IpDn<'t>,
    last_will: Option<L>,
    username: Option<&'t str>,
    password: Option<&'t str>,
    subscriptions: [Topic<T>; S],
}

impl<'t, T: Deref<Target = str> + 't, L: Publishable + 't> McutieBuilder<'t, T, L, 0> {
    /// Creates a new builder with the initial required configuration.
    ///
    /// `device_type` is expected to be the same for all devices of the same type.
    /// `broker` may be an IP address or a DNS name for the broker to connect to.
    pub fn new(network: Stack<'t>, device_id: &'t str, broker: IpDn<'t>,tcp_time_out: u64,tcp_keep_alive: u64,mqtt_ping_secs: u16,mqtt_keep_alive: u16) -> Self {
        Self {
            network,
            tcp_time_out,
            tcp_keep_alive,
            mqtt_ping_secs,
            mqtt_keep_alive,
            broker,
            device_id,
            last_will: None,
            username: None,
            password: None,
            subscriptions: [],
        }
    }
}

impl<'t, T: Deref<Target = str> + 't, L: Publishable + 't, const S: usize>
    McutieBuilder<'t, T, L, S>
{
    /// Add some default topics to subscribe to.
    pub fn with_subscriptions<const N: usize>(
        self,
        subscriptions: [Topic<T>; N],
    ) -> McutieBuilder<'t, T, L, N> {
        McutieBuilder {
            network: self.network,
            tcp_time_out: self.tcp_time_out,
            tcp_keep_alive: self.tcp_keep_alive,
            mqtt_ping_secs: self.mqtt_ping_secs,
            mqtt_keep_alive: self.mqtt_keep_alive,
            broker: self.broker,
            device_id: self.device_id,
            last_will: self.last_will,
            username: self.username,
            password: self.password,
            subscriptions,
        }
    }
}

impl<'t, T: Deref<Target = str> + 't, L: Publishable + 't, const S: usize>
    McutieBuilder<'t, T, L, S>
{
    /// Adds authentication for the broker.
    pub fn with_authentication(self, username: &'t str, password: &'t str) -> Self {
        Self {
            username: Some(username),
            password: Some(password),
            ..self
        }
    }

    /// Sets a last will message to be published in the event of disconnection.
    pub fn with_last_will(self, last_will: L) -> Self {
        Self {
            last_will: Some(last_will),
            ..self
        }
    }

    /// Initialises the MQTT stack returning a receiver for listening to
    /// messages from the broker and a future that must be run in order for the
    /// stack to operate.
    pub fn build<'a>(self, sender: &'a McutieSender,receive: &'a McutieReceiver) -> McutieTask<'t,'a, T, L, S> {
        McutieTask {
            network: self.network,
            broker: self.broker,
            tcp_time_out: self.tcp_time_out,
            tcp_keep_alive: self.tcp_keep_alive,
            mqtt_ping_secs: self.mqtt_ping_secs,
            mqtt_keep_alive: self.mqtt_keep_alive,
            client_id: self.device_id,
            last_will: self.last_will,
            username: self.username,
            password: self.password,
            subscriptions: self.subscriptions,
            receive,
            sender
        }
    }
}
