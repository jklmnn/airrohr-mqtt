#[macro_use] extern crate rocket;
use rocket::http::Status;
use serde_json::json;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct SensorDataValue {
    value_type: String,
    value: String
}

#[derive(Debug, Deserialize)]
struct Device {
    esp8266id: String,
    software_version: String,
    sensordatavalues: Vec<SensorDataValue>
}

#[post("/api/<sensor>/<key>", data="<data>")]
fn api(sensor: &str, key: &str, data: &str) -> Status {
    let device_measurement: Device = match serde_json::from_str(data) {
        Ok(dev) => dev,
        Err(e) => {
            println!("{e}");
            return Status::BadRequest;
        }
    };
    println!("{sensor}:{key}\n{data}");
    Status::Ok
}

#[launch]
fn server() -> _{
    rocket::build()
        .mount("/", routes![api])
}
