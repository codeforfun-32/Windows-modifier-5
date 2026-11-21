use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use serde_json::Value;
use slint::Weak;
use windows::Devices::Geolocation::Geolocator;
use windows::Foundation::TimeSpan;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

use crate::app_state::WEATHER_CITY;
use crate::links::{http_get_string, url_encode};
use crate::{MainWindow, WxItem};

struct Report {
    city: String,
    temp: i32,
    feels: i32,
    hi: i32,
    lo: i32,
    humidity: i32,
    wind: i32,
    rain: i32,
    sunset: String,
    kind: i32,
    desc: &'static str,
    hours: Vec<(String, i32, String, String)>,
    days: Vec<(String, i32, String, String)>,
}

static REFRESH: AtomicBool = AtomicBool::new(false);

/// Ask for a new forecast right now.
pub fn refresh() {
    REFRESH.store(true, Ordering::Relaxed);
}

/// "2026-11-19T17:26" -> "5:26 PM"
fn clock12(iso: &str) -> String {
    let t = iso.split('T').nth(1).unwrap_or("");
    let h: i32 = t.get(0..2).and_then(|x| x.parse().ok()).unwrap_or(0);
    let m = t.get(3..5).unwrap_or("00");
    let (h12, ap) = match h {
        0 => (12, "AM"),
        1..=11 => (h, "AM"),
        12 => (12, "PM"),
        _ => (h - 12, "PM"),
    };
    format!("{h12}:{m} {ap}")
}

/// "2026-11-19T17" -> "5 PM"
fn hour_label(iso: &str) -> String {
    let h: i32 = iso.split('T').nth(1).and_then(|t| t.get(0..2)).and_then(|x| x.parse().ok()).unwrap_or(0);
    match h {
        0 => "12 AM".to_string(),
        1..=11 => format!("{h} AM"),
        12 => "12 PM".to_string(),
        _ => format!("{} PM", h - 12),
    }
}

/// "2026-11-19" -> "Thu"
fn weekday(date: &str) -> &'static str {
    let mut it = date.split('-').map(|x| x.parse::<i32>().unwrap_or(1));
    let (y, m, d) = (it.next().unwrap_or(2026), it.next().unwrap_or(1), it.next().unwrap_or(1));
    // Sakamoto's method: 0 = Sunday
    let t = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let yy = if m < 3 { y - 1 } else { y };
    let w = (yy + yy / 4 - yy / 100 + yy / 400 + t[(m.clamp(1, 12) - 1) as usize] + d) % 7;
    ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][w as usize]
}

fn night(kind: i32, is_day: bool) -> i32 {
    if is_day {
        kind
    } else {
        match kind {
            0 => 7,
            1 | 2 => 8,
            k => k,
        }
    }
}

#[derive(Clone)]
struct Place {
    lat: f64,
    lon: f64,
    name: String,
}

fn describe(code: i64) -> (i32, &'static str) {
    match code {
        0 => (0, "Clear sky"),
        1 => (0, "Mostly clear"),
        2 => (1, "Partly cloudy"),
        3 => (2, "Overcast"),
        45 | 48 => (6, "Fog"),
        51 | 53 | 55 | 56 | 57 => (3, "Drizzle"),
        61 | 63 | 65 | 66 | 67 => (3, "Rain"),
        80 | 81 | 82 => (3, "Showers"),
        71 | 73 | 75 | 77 | 85 | 86 => (4, "Snow"),
        95 | 96 | 99 => (5, "Thunderstorm"),
        _ => (2, "Cloudy"),
    }
}

fn place_name(lat: f64, lon: f64) -> String {
    let url = format!(
        "https://api.bigdatacloud.net/data/reverse-geocode-client?latitude={lat}&longitude={lon}&localityLanguage=en"
    );
    let parsed: Option<Value> = http_get_string(&url).and_then(|t| serde_json::from_str(&t).ok());
    if let Some(v) = parsed {
        for key in ["city", "locality", "principalSubdivision"] {
            if let Some(s) = v[key].as_str() {
                if !s.is_empty() {
                    return s.to_string();
                }
            }
        }
    }
    "Your location".to_string()
}

fn windows_place() -> Option<Place> {
    let geo = Geolocator::new().ok()?;
    let max_age = TimeSpan { Duration: 10 * 60 * 10_000_000 };
    let timeout = TimeSpan { Duration: 8 * 10_000_000 };
    let pos = geo
        .GetGeopositionAsyncWithAgeAndTimeout(max_age, timeout)
        .ok()?
        .join()
        .ok()?;
    let point = pos.Coordinate().ok()?.Point().ok()?.Position().ok()?;
    let lat = point.Latitude;
    let lon = point.Longitude;
    Some(Place { lat, lon, name: place_name(lat, lon) })
}

fn ip_place() -> Option<Place> {
    let text = http_get_string("https://ipwho.is/")?;
    let v: Value = serde_json::from_str(&text).ok()?;
    if v["success"].as_bool() == Some(false) {
        return None;
    }
    Some(Place {
        lat: v["latitude"].as_f64()?,
        lon: v["longitude"].as_f64()?,
        name: v["city"].as_str().unwrap_or("Your location").to_string(),
    })
}

fn city_place(city: &str) -> Option<Place> {
    let geo_url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=1",
        url_encode(city)
    );
    let geo: Value = serde_json::from_str(&http_get_string(&geo_url)?).ok()?;
    let place = geo["results"].get(0)?;
    Some(Place {
        lat: place["latitude"].as_f64()?,
        lon: place["longitude"].as_f64()?,
        name: place["name"].as_str().unwrap_or(city).to_string(),
    })
}

fn forecast(place: &Place) -> Option<Report> {
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}\
         &current=temperature_2m,relative_humidity_2m,apparent_temperature,weather_code,wind_speed_10m,is_day\
         &hourly=temperature_2m,weather_code,is_day,precipitation_probability\
         &daily=temperature_2m_max,temperature_2m_min,weather_code,sunset,precipitation_probability_max\
         &forecast_days=7&timezone=auto",
        place.lat, place.lon
    );
    let data: Value = serde_json::from_str(&http_get_string(&url)?).ok()?;
    let cur = &data["current"];
    let (kind0, desc) = describe(cur["weather_code"].as_i64()?);
    let is_day_now = cur["is_day"].as_i64().unwrap_or(1) == 1;

    // hours: from the current hour, 24 of them
    let now_hour = cur["time"].as_str().unwrap_or("").get(0..13).unwrap_or("").to_string();
    let times = data["hourly"]["time"].as_array()?;
    let start = times
        .iter()
        .position(|t| t.as_str().map_or(false, |s| s.get(0..13) == Some(now_hour.as_str())))
        .unwrap_or(0);
    let mut hours = Vec::new();
    let mut rain_now = 0;
    for (n, i) in (start..times.len()).take(24).enumerate() {
        let t = times[i].as_str().unwrap_or("");
        let temp = data["hourly"]["temperature_2m"].get(i)?.as_f64().unwrap_or(0.0).round() as i32;
        let code = data["hourly"]["weather_code"].get(i)?.as_i64().unwrap_or(3);
        let day = data["hourly"]["is_day"].get(i).and_then(|v| v.as_i64()).unwrap_or(1) == 1;
        let pr = data["hourly"]["precipitation_probability"].get(i).and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        if n == 0 {
            rain_now = pr;
        }
        let label = if n == 0 { "Now".to_string() } else { hour_label(t) };
        hours.push((label, night(describe(code).0, day), format!("{temp}°"), format!("{pr}%")));
    }

    let dtimes = data["daily"]["time"].as_array()?;
    let mut days = Vec::new();
    for (i, t) in dtimes.iter().enumerate().take(7) {
        let hi = data["daily"]["temperature_2m_max"].get(i)?.as_f64().unwrap_or(0.0).round() as i32;
        let lo = data["daily"]["temperature_2m_min"].get(i)?.as_f64().unwrap_or(0.0).round() as i32;
        let code = data["daily"]["weather_code"].get(i)?.as_i64().unwrap_or(3);
        let label = if i == 0 { "Today".to_string() } else { weekday(t.as_str().unwrap_or("")).to_string() };
        days.push((label, describe(code).0, format!("{hi}°"), format!("{lo}°")));
    }

    Some(Report {
        city: place.name.clone(),
        temp: cur["temperature_2m"].as_f64()?.round() as i32,
        feels: cur["apparent_temperature"].as_f64().unwrap_or(0.0).round() as i32,
        humidity: cur["relative_humidity_2m"].as_f64()?.round() as i32,
        wind: cur["wind_speed_10m"].as_f64()?.round() as i32,
        hi: data["daily"]["temperature_2m_max"].get(0)?.as_f64()?.round() as i32,
        lo: data["daily"]["temperature_2m_min"].get(0)?.as_f64()?.round() as i32,
        rain: rain_now,
        sunset: clock12(data["daily"]["sunset"].get(0).and_then(|v| v.as_str()).unwrap_or("")),
        kind: night(kind0, is_day_now),
        desc,
        hours,
        days,
    })
}

pub fn spawn(ui: Weak<MainWindow>) {
    thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let mut last_place: Option<Place> = None;

        loop {
            let place = windows_place()
                .or_else(ip_place)
                .or_else(|| last_place.clone())
                .or_else(|| city_place(WEATHER_CITY));

            let wait_secs = match place.as_ref().and_then(forecast) {
                Some(r) => {
                    last_place = place;
                    let sent = ui.upgrade_in_event_loop(move |ui| {
                        ui.set_weather_city(r.city.into());
                        ui.set_weather_temp(format!("{}°", r.temp).into());
                        ui.set_weather_desc(r.desc.into());
                        ui.set_weather_hilo(format!("H {}° · L {}° · Feels {}°", r.hi, r.lo, r.feels).into());
                        ui.set_weather_rain(format!("{}%", r.rain).into());
                        ui.set_weather_sunset(r.sunset.into());
                        let conv = |v: Vec<(String, i32, String, String)>| -> slint::ModelRc<WxItem> {
                            let items: Vec<WxItem> = v
                                .into_iter()
                                .map(|(label, kind, temp, sub)| WxItem {
                                    label: label.into(),
                                    kind,
                                    temp: temp.into(),
                                    sub: sub.into(),
                                })
                                .collect();
                            slint::ModelRc::new(slint::VecModel::from(items))
                        };
                        ui.set_weather_hours(conv(r.hours));
                        ui.set_weather_days(conv(r.days));
                        ui.set_weather_humidity(format!("{}%", r.humidity).into());
                        ui.set_weather_wind(format!("{} km/h", r.wind).into());
                        ui.set_weather_kind(r.kind);
                        ui.set_weather_ready(true);
                    });
                    if sent.is_err() {
                        return;
                    }
                    900
                }
                None => 60,
            };
            for _ in 0..wait_secs {
                if REFRESH.swap(false, Ordering::Relaxed) {
                    break;
                }
                thread::sleep(Duration::from_secs(1));
            }
        }
    });
}