# mcutie

[![Tests](https://github.com/Mossop/mcutie/actions/workflows/test.yml/badge.svg)](https://github.com/Mossop/mcutie/actions/workflows/test.yml)
[![Latest Version](https://img.shields.io/crates/v/mcutie.svg)](https://crates.io/crates/mcutie)
[![Documentation](https://docs.rs/mcutie/badge.svg)](https://docs.rs/mcutie)

A simple MQTT client designed for use in embedded devices using the `embassy-net` networking stack.
Requires an async executor. Runs in `no-std` contexts and includes specific support for Home
Assistant's device auto-discovery.

The stack is designed to be a singleton for the lifetime of the application. Once initialised it
runs forever attempting to maintain a connection to the MQTT broker. It includes support for
automatically subscribing to some topics when connected to the broker and registering a last will
message to be published if the connection to the broker it lost.

The different MQTT QoS levels are supported to a certain extent. In most cases a QoS of 0 is used by
default which means your message may never make it to the broker. This is particularly true in the
case where the network is disconnected or the broker is unreachable in which case you will get no
error or other warning after publishing a message. If you need that you can set a higher QoS level.
This crate will not automatically re-send messages that fail to be delivered however you will get an
error if the broker does not acknowledge messages within a certain time (currently 2 seconds).
