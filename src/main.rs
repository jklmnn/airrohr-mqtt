#[macro_use] extern crate rocket;
use rocket::{http::Status, State};
use serde_json;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::{Arc, Mutex}, env, time::Duration};
use paho_mqtt::{Client, ConnectOptionsBuilder, Message};
use phf::phf_map;

struct Sensor {
    class: &'static str,
    unit: &'static str
}

static SENSORS: phf::Map<&'static str, Sensor> = phf_map! {
    "BME280_temperature" => Sensor {class: "temperature", unit: "C"},
};

#[derive(Debug, Deserialize)]
struct SensorDataValue {
    value_type: String,
    value: String
}

#[derive(Debug, Deserialize)]
struct Airrohr {
    esp8266id: String,
    software_version: String,
}

#[derive(Debug, Deserialize)]
struct Measurement {
    #[serde(flatten)]
    airrohr: Airrohr,
    sensordatavalues: Vec<SensorDataValue>
}

#[derive(Debug, Serialize)]
struct Device {
    identifiers: Vec<String>,
    manufacturer: String,
    model: String,
    name: String,
    sw_version: String
}

#[derive(Debug, Serialize)]
struct Entity {
    name: String,
    state_topic: String,
    unique_id: String,
    device_class: String,
    unit_of_measurement: String,
    value_template: String,
}

#[derive(Debug, Serialize)]
struct Config {
    device: Device,
    #[serde(flatten)]
    entity: Entity
}

impl SensorDataValue {
    fn supported(&self) -> bool {
        SENSORS.contains_key(&self.value_type)
    }

    fn class(&self) -> Option<String> {
        Some(String::from(SENSORS.get(&self.value_type)?.class))
    }

    fn unit(&self) -> Option<String> {
        Some(String::from(SENSORS.get(&self.value_type)?.unit))
    }
}

impl Airrohr {
    fn name(&self) -> String {
        format!("airrohr-{}", self.esp8266id)
    }

    fn state_topic(&self, sdv: &SensorDataValue) -> String {
        String::from(format!("airrohr/{}/{}", self.name(), sdv.value_type))
    }
}

impl Entity {
    fn new(a: &Airrohr, sdv: &SensorDataValue) -> Option<Entity> {
        let id_name = String::from(format!("{}-{}", a.name(), sdv.value_type));
        Some(Entity {
            name: id_name.clone(),
            state_topic: a.state_topic(sdv),
            unique_id: id_name,
            device_class: sdv.class()?,
            unit_of_measurement: sdv.unit()?,
            value_template: String::from("{{ value }}")
        })
    }
}

impl Device {
    fn new(a: &Airrohr) -> Device {
        Device {
            identifiers: vec![a.name(),
                              String::from(format!("Feinstaubsensor-{}", a.esp8266id)),
                              String::from(format!("Particulate Matter {}", a.esp8266id))],
            manufacturer: String::from("Open Knowledge Lab Stuttgart a.o. (Code for Germany)"),
            model: String::from("Particulate matter sensor"),
            name: a.name(),
            sw_version: a.software_version.clone()
        }
    }
}

struct BridgeDev {
    key: String,
    seen: bool
}

impl BridgeDev {
    fn new(key: &str) -> BridgeDev {
        BridgeDev {
            key: String::from(key),
            seen: false
        }
    }
}

struct Bridge {
    devices: HashMap<String, BridgeDev>,
    mqtt: Client,
}

type BridgeReference = Arc<Mutex<Bridge>>;

impl Bridge {
    fn new(mqtturi: &str, user: &str, password: &str) -> Bridge {
        println!("{mqtturi}");
        let mqtt = Client::new(mqtturi).unwrap();
        let conn_opts = ConnectOptionsBuilder::new()
            .keep_alive_interval(Duration::from_secs(20))
            .clean_session(true)
            .user_name(user)
            .password(password)
            .finalize();
        mqtt.connect(conn_opts).unwrap();
        Bridge {
            devices: HashMap::<String, BridgeDev>::new(),
            mqtt
        }
    }

    fn authorize(&mut self, measurement: &Measurement, key: &str) -> bool {
        let name = measurement.airrohr.name();
        match self.devices.get(&name) {
            Some(k) => {
                k.key.eq(key)
            }
            None => {
                self.devices.insert(name.clone(), BridgeDev::new(key));
                true
                // TODO: load device names and key from config, until then TOFU
            }
        }
    }

    fn seen(&self, measurement: &Measurement) -> bool {
        match self.devices.get(&measurement.airrohr.name()) {
            Some(b) => b.seen,
            None => false
        }
    }

    fn advertise(&mut self, a: &Airrohr, v: &SensorDataValue) -> bool {
        let config = Config {
            device: Device::new(a),
            entity: match Entity::new(a, v) {
                Some(e) => e,
                None => return false
            }
        };
        let json_str = match serde_json::to_string(&config) {
            Ok(s) => s,
            Err(_) => return true
        };
        let result = self.mqtt.publish(Message::new(format!("homeassistant/sensor/{}/config", &a.name()), json_str, 1)).is_err();
        if let Some(b) = self.devices.get_mut(&a.name()) {
            b.seen = true;
        }
        result
    }

    fn send_data(&self, a: &Airrohr, v: &SensorDataValue) -> bool {
        self.mqtt.publish(Message::new(a.state_topic(&v), v.value.clone(), 0)).is_err()
    }
}

#[post("/api/<key>", data="<data>")]
fn api(dev_ref: &State<BridgeReference>, key: &str, data: &str) -> Status {
    let mut devices = match dev_ref.lock() {
        Ok(dev) => dev,
        Err(_) => return Status::InternalServerError
    };
    let device_measurement: Measurement = match serde_json::from_str(data) {
        Ok(dev) => dev,
        Err(_) => {
            return Status::BadRequest;
        }
    };
    if !devices.authorize(&device_measurement, key) {
        return Status::Unauthorized;
    }
    for v in &device_measurement.sensordatavalues {
        if !v.supported() {
            continue;
        }
        if !devices.seen(&device_measurement) {
            if devices.advertise(&device_measurement.airrohr, &v) {
                return Status::InternalServerError
            }
        }
        if devices.send_data(&device_measurement.airrohr, &v) {
            return Status::InternalServerError
        }
    }
    Status::Ok
}

#[launch]
fn server() -> _{
    let args: Vec<String> = env::args().collect();
    let bridge = Bridge::new(&args[1], &args[2], &args[3]);
    rocket::build()
        .mount("/", routes![api])
        .manage(Arc::new(Mutex::new(bridge)))
}
