#[macro_use]
extern crate rocket;
use paho_mqtt::{Client, ConnectOptionsBuilder, Message};
use rocket::{http::Status, State};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::BufReader,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Debug, Deserialize)]
struct Sensor {
    class: String,
    unit: String,
    value_template: String,
}

#[derive(Debug, Deserialize)]
struct SensorDataValue {
    value_type: String,
    value: String,
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
    sensordatavalues: Vec<SensorDataValue>,
}

#[derive(Debug, Serialize)]
struct Device {
    identifiers: Vec<String>,
    manufacturer: String,
    model: String,
    name: String,
    sw_version: String,
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
    entity: Entity,
}

impl Airrohr {
    fn name(&self) -> String {
        format!("airrohr-{}", self.esp8266id)
    }

    fn state_topic(&self, sensor: &str) -> String {
        format!("airrohr/{}/{}", self.name(), sensor)
    }
}

impl Entity {
    fn new(
        a: &Airrohr,
        sensor: &str,
        device_class: Option<String>,
        unit_of_measurement: Option<String>,
        value_template: Option<String>,
    ) -> Option<Entity> {
        let id_name = format!("{}-{}", a.name(), sensor);
        Some(Entity {
            name: id_name.clone(),
            state_topic: a.state_topic(sensor),
            unique_id: id_name,
            device_class: device_class?,
            unit_of_measurement: unit_of_measurement?,
            value_template: value_template?,
        })
    }
}

impl Device {
    fn new(a: &Airrohr) -> Device {
        Device {
            identifiers: vec![
                a.name(),
                format!("Feinstaubsensor-{}", a.esp8266id),
                format!("Particulate Matter {}", a.esp8266id),
            ],
            manufacturer: String::from("Open Knowledge Lab Stuttgart a.o. (Code for Germany)"),
            model: String::from("Particulate matter sensor"),
            name: a.name(),
            sw_version: a.software_version.clone(),
        }
    }
}

struct BridgeDev {
    sensors: HashSet<String>,
}

impl BridgeDev {
    fn new() -> BridgeDev {
        BridgeDev {
            sensors: HashSet::new(),
        }
    }
}

struct Bridge {
    devices: HashMap<String, BridgeDev>,
    mqtt: Client,
    sensors: HashMap<String, Sensor>,
}

type BridgeReference = Arc<Mutex<Bridge>>;

impl Bridge {
    fn new(mqtturi: &str, user: &str, password: &str, sensors: HashMap<String, Sensor>) -> Bridge {
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
            mqtt,
            sensors,
        }
    }

    fn update_device(&mut self, measurement: &Measurement) {
        let name = measurement.airrohr.name();
        self.devices.entry(name).or_insert_with(BridgeDev::new);
    }

    fn seen(&self, a: &Airrohr, sdv: &SensorDataValue) -> bool {
        match self.devices.get(&a.name()) {
            Some(b) => b.sensors.contains(&sdv.value_type),
            None => false,
        }
    }

    fn supported(&self, v: &SensorDataValue) -> bool {
        self.sensors.contains_key(&v.value_type)
    }

    fn device_class(&self, v: &SensorDataValue) -> Option<String> {
        Some(self.sensors.get(&v.value_type)?.class.clone())
    }

    fn unit_of_measurement(&self, v: &SensorDataValue) -> Option<String> {
        Some(self.sensors.get(&v.value_type)?.unit.clone())
    }

    fn value_template(&self, v: &SensorDataValue) -> Option<String> {
        Some(self.sensors.get(&v.value_type)?.value_template.clone())
    }

    fn advertise(&mut self, a: &Airrohr, v: &SensorDataValue) -> bool {
        let config = Config {
            device: Device::new(a),
            entity: match Entity::new(
                a,
                &v.value_type,
                self.device_class(v),
                self.unit_of_measurement(v),
                self.value_template(v),
            ) {
                Some(e) => e,
                None => return false,
            },
        };
        let json_str = match serde_json::to_string(&config) {
            Ok(s) => s,
            Err(_) => return true,
        };
        let result = self
            .mqtt
            .publish(Message::new(
                format!("homeassistant/sensor/{}/{}/config", &a.name(), v.value_type),
                json_str,
                1,
            ))
            .is_err();
        if let Some(b) = self.devices.get_mut(&a.name()) {
            b.sensors.insert(v.value_type.clone());
        }
        result
    }

    fn send_data(&self, a: &Airrohr, v: &SensorDataValue) -> bool {
        self.mqtt
            .publish(Message::new(
                a.state_topic(&v.value_type),
                v.value.clone(),
                0,
            ))
            .is_err()
    }
}

#[post("/api", data = "<data>")]
fn api(dev_ref: &State<BridgeReference>, data: &str) -> Status {
    let mut devices = match dev_ref.lock() {
        Ok(dev) => dev,
        Err(_) => return Status::InternalServerError,
    };
    let device_measurement: Measurement = match serde_json::from_str(data) {
        Ok(dev) => dev,
        Err(_) => {
            return Status::BadRequest;
        }
    };
    devices.update_device(&device_measurement);
    for v in &device_measurement.sensordatavalues {
        if !devices.supported(v) {
            continue;
        }
        if (!devices.seen(&device_measurement.airrohr, v)
            && devices.advertise(&device_measurement.airrohr, v))
            || devices.send_data(&device_measurement.airrohr, v)
        {
            return Status::InternalServerError;
        }
    }
    Status::Ok
}

#[launch]
fn server() -> _ {
    let settings = config::Config::builder()
        .add_source(config::File::with_name("Settings"))
        .build()
        .expect("failed to read Settings.toml");
    let file = File::open(
        &settings
            .get_string("sensors")
            .expect("failed to get sensor definitions"),
    )
    .expect("unable to open def file");
    let bridge = Bridge::new(
        &settings
            .get_string("server")
            .expect("failed to get server from settings"),
        &settings
            .get_string("user")
            .expect("failed to get user from settings"),
        &settings
            .get_string("password")
            .expect("failed to get password from settings"),
        serde_json::from_reader(BufReader::new(file)).expect("failed to parse definitions"),
    );
    rocket::build()
        .mount("/", routes![api])
        .manage(Arc::new(Mutex::new(bridge)))
}
