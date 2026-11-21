//! Print the box's MQTT topic root (`<prefix>/<client_id>`) and broker, the
//! same answer every openHC daemon gets from `ohcmqtt`, for init scripts that
//! configure a program which is not one of ours (the radio bridges).
//!
//! `ohc-mqtt-base`         -> `openhc/<host>`
//! `ohc-mqtt-base broker`  -> `127.0.0.1 1883`

fn main() {
    if std::env::args().nth(1).as_deref() == Some("broker") {
        let (host, port) = ohcmqtt::broker();
        println!("{host} {port}");
    } else {
        println!("{}", ohcmqtt::Base::resolve().root());
    }
}
