#[macro_use] extern crate rocket;
use rocket::{http::Status, State};
use serde_json::json;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::{Arc, Mutex}};

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

fn sensor_properties(sensor: &str) -> Option<(String, String)> {
    match sensor {
        "BME280_temperature" => Some((String::from("temperature"), String::from("C"))),
        _ => None
    }
}

impl Airrohr {
    fn id(&self) -> String {
        self.esp8266id.clone()
    }

    fn name(&self) -> String {
        format!("airrohr-{}", self.esp8266id)
    }
}

impl Entity {
    fn new(a: &Airrohr, sdv: &SensorDataValue) -> Option<Entity> {
        let (dev_class, unit)= sensor_properties(&sdv.value_type)?;
        let id_name = String::from(format!("{}-{}", a.name(), sdv.value_type));
        Some(Entity {
            name: id_name.clone(),
            state_topic: String::from(format!("airrohr/{}/{}", a.name(), sdv.value_type)),
            unique_id: id_name,
            device_class: dev_class,
            unit_of_measurement: unit,
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

struct Bridge {
    devices: HashMap<String, String>,
}

type BridgeReference = Arc<Mutex<Bridge>>;

impl Bridge {
    fn new() -> BridgeReference {
        let bridge = Bridge {
            devices: HashMap::<String, String>::new()
        };
        Arc::new(Mutex::new(bridge))
    }

    fn authorize(&mut self, measurement: &Measurement, key: &str) -> bool {
        match self.devices.get(&measurement.airrohr.name()) {
            Some(k) => {
                k.eq(key)
            }
            None => {
                self.devices.insert(measurement.airrohr.name(), String::from(key));
                let device = Device::new(&measurement.airrohr);
                true
            }
        }
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
        Err(e) => {
            println!("{e}");
            return Status::BadRequest;
        }
    };
    if !devices.authorize(&device_measurement, key) {
        return Status::Unauthorized;
    }
    Status::Ok
}

#[launch]
fn server() -> _{
    rocket::build()
        .mount("/", routes![api])
        .manage(Bridge::new())
}
