//! Weather from Open-Meteo (no API key). Location resolution follows
//! Caelestia's `services.weatherLocation`: "lat,long", a city name
//! (Open-Meteo geocoding) or empty for IP-based geolocation.

use std::{thread, time::Duration};

use crossbeam_channel::{Sender, unbounded};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct WeatherQuery {
    /// UI language: part of the query so a language change refetches.
    pub lang: String,
    pub location: String,
    /// Display name picked by the user (coordinates don't carry one).
    pub name: String,
    pub fahrenheit: bool,
    pub twelve_hour: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Day {
    pub day: String,
    pub icon: String,
    pub max: String,
    pub min: String,
}

#[derive(Debug, Clone, Default)]
pub struct WeatherSnapshot {
    pub city: String,
    pub temp: String,
    pub feels: String,
    pub icon: String,
    pub description: String,
    pub humidity: String,
    pub wind: String,
    pub precipitation: String,
    pub sunrise: String,
    pub sunset: String,
    pub forecast: Vec<Day>,
}

pub struct WeatherService {
    tx: Sender<WeatherQuery>,
}

impl WeatherService {
    pub fn spawn(initial: WeatherQuery, on_update: impl Fn(Option<WeatherSnapshot>) + Send + 'static) -> Self {
        let (tx, rx) = unbounded::<WeatherQuery>();
        thread::Builder::new()
            .name("weather".into())
            .spawn(move || {
                let mut q = initial;
                loop {
                    match fetch(&q) {
                        Ok(w) => on_update(Some(w)),
                        Err(e) => {
                            log::warn!("weather: {e:#}");
                            on_update(None);
                        }
                    }
                    // Refresh every 15 minutes or as soon as the settings change.
                    match rx.recv_timeout(Duration::from_secs(15 * 60)) {
                        Ok(newq) => q = rx.try_iter().last().unwrap_or(newq),
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
                        Err(_) => {}
                    }
                }
            })
            .expect("spawn weather");
        Self { tx }
    }

    pub fn update(&self, q: WeatherQuery) {
        let _ = self.tx.send(q);
    }

    /// Re-fetches now with the given (usually unchanged) query.
    pub fn refresh(&self, q: WeatherQuery) {
        self.update(q);
    }
}

fn get_json(url: &str) -> anyhow::Result<Value> {
    let mut resp = ureq::get(url).header("User-Agent", "caelestia-win").call()?;
    Ok(resp.body_mut().read_json::<Value>()?)
}

fn resolve(location: &str) -> anyhow::Result<(f64, f64, String)> {
    let loc = location.trim();
    if let Some((a, b)) = loc.split_once(',') {
        if let (Ok(lat), Ok(lon)) = (a.trim().parse(), b.trim().parse()) {
            return Ok((lat, lon, String::new()));
        }
    }
    if !loc.is_empty() {
        let url = format!("https://geocoding-api.open-meteo.com/v1/search?count=1&name={}", urlencode(loc));
        let v = get_json(&url)?;
        let r = v["results"].get(0).ok_or_else(|| anyhow::anyhow!("location '{loc}' not found"))?;
        return Ok((r["latitude"].as_f64().unwrap_or(0.0), r["longitude"].as_f64().unwrap_or(0.0), r["name"].as_str().unwrap_or(loc).to_owned()));
    }
    let v = get_json("https://ipinfo.io/json")?;
    let (lat, lon) = v["loc"].as_str().and_then(|s| s.split_once(',')).ok_or_else(|| anyhow::anyhow!("no IP location"))?;
    Ok((lat.parse()?, lon.parse()?, v["city"].as_str().unwrap_or_default().to_owned()))
}

fn fetch(q: &WeatherQuery) -> anyhow::Result<WeatherSnapshot> {
    let (lat, lon, city) = resolve(&q.location)?;
    let city = if q.name.is_empty() { city } else { q.name.clone() };
    let unit = if q.fahrenheit { "&temperature_unit=fahrenheit&wind_speed_unit=mph" } else { "" };
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}\
         &current=temperature_2m,apparent_temperature,relative_humidity_2m,precipitation,weather_code,wind_speed_10m,is_day\
         &daily=weather_code,temperature_2m_max,temperature_2m_min,sunrise,sunset&timezone=auto&forecast_days=7{unit}"
    );
    let v = get_json(&url)?;
    let cur = &v["current"];
    let deg = if q.fahrenheit { "°F" } else { "°C" };
    let code = cur["weather_code"].as_i64().unwrap_or(0);
    let is_day = cur["is_day"].as_i64().unwrap_or(1) == 1;
    let t = |x: &Value| format!("{}{deg}", x.as_f64().unwrap_or(0.0).round());
    let daily = &v["daily"];
    let time_of = |s: &Value| {
        let s = s.as_str().unwrap_or_default();
        let hm = s.split('T').nth(1).unwrap_or(s);
        if q.twelve_hour {
            if let Some((h, m)) = hm.split_once(':') {
                let h: u32 = h.parse().unwrap_or(0);
                return format!("{}:{m} {}", if h % 12 == 0 { 12 } else { h % 12 }, if h < 12 { "am" } else { "pm" });
            }
        }
        hm.to_owned()
    };
    let mut forecast = Vec::new();
    if let Some(days) = daily["time"].as_array() {
        for (i, d) in days.iter().enumerate() {
            let date = chrono::NaiveDate::parse_from_str(d.as_str().unwrap_or_default(), "%Y-%m-%d").ok();
            forecast.push(Day {
                day: if i == 0 {
                    crate::i18n::tr("Today")
                } else {
                    date.map(|d| crate::i18n::capitalize(&d.format_localized("%a", crate::i18n::chrono_locale()).to_string())).unwrap_or_default()
                },
                icon: icon(daily["weather_code"][i].as_i64().unwrap_or(0), true).into(),
                max: t(&daily["temperature_2m_max"][i]),
                min: t(&daily["temperature_2m_min"][i]),
            });
        }
    }
    Ok(WeatherSnapshot {
        city,
        temp: t(&cur["temperature_2m"]),
        feels: t(&cur["apparent_temperature"]),
        icon: icon(code, is_day).into(),
        description: crate::i18n::tr(description(code)),
        humidity: format!("{}%", cur["relative_humidity_2m"].as_f64().unwrap_or(0.0).round()),
        wind: format!("{} {}", cur["wind_speed_10m"].as_f64().unwrap_or(0.0).round(), if q.fahrenheit { "mph" } else { "km/h" }),
        precipitation: format!("{} {}", cur["precipitation"].as_f64().unwrap_or(0.0), if q.fahrenheit { "in" } else { "mm" }),
        sunrise: time_of(&daily["sunrise"][0]),
        sunset: time_of(&daily["sunset"][0]),
        forecast,
    })
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// WMO weather code -> Material Symbols icon (same mapping as Caelestia).
pub fn icon(code: i64, day: bool) -> &'static str {
    match code {
        0 => if day { "clear_day" } else { "clear_night" },
        1 | 2 => if day { "partly_cloudy_day" } else { "partly_cloudy_night" },
        3 => "cloud",
        45 | 48 => "foggy",
        51..=57 => "rainy_light",
        61..=67 | 80..=82 => "rainy",
        71..=77 | 85 | 86 => "weather_snowy",
        95..=99 => "thunderstorm",
        _ => "cloud",
    }
}

pub fn description(code: i64) -> &'static str {
    match code {
        0 => "Clear sky",
        1 => "Mainly clear",
        2 => "Partly cloudy",
        3 => "Overcast",
        45 | 48 => "Fog",
        51 | 53 | 55 => "Drizzle",
        56 | 57 => "Freezing drizzle",
        61 | 63 | 65 => "Rain",
        66 | 67 => "Freezing rain",
        71 | 73 | 75 | 77 => "Snow",
        80..=82 => "Rain showers",
        85 | 86 => "Snow showers",
        95 => "Thunderstorm",
        96 | 99 => "Thunderstorm with hail",
        _ => "Unknown",
    }
}

// ----------------------------------------------------------------------
// Location picking (Settings › Services): search, detect, name.
// ----------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub name: String,
    /// "Region, Country".
    pub detail: String,
    pub country_code: String,
    pub lat: f64,
    pub lon: f64,
}

impl Place {
    /// Value stored in `services.weatherLocation` (exact, no re-geocoding).
    pub fn coords(&self) -> String {
        format!("{:.4},{:.4}", self.lat, self.lon)
    }
    pub fn label(&self) -> String {
        if self.detail.is_empty() { self.name.clone() } else { format!("{}, {}", self.name, self.detail) }
    }
}

/// Lowercase without accents, for forgiving matching ("Mexico" == "México").
fn fold(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' | 'ã' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' | 'õ' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            other => other,
        })
        .collect()
}

/// City search with Open-Meteo's geocoder (no key), localised names.
///
/// * "City, Region/Country" works: the part after the comma filters results
///   (the geocoder itself only matches names).
/// * Places in `home_country` (the Windows region, e.g. "CR") come first.
pub fn search_places(query: &str, lang: &str, home_country: &str) -> anyhow::Result<Vec<Place>> {
    let q = query.trim();
    let (name, filter) = match q.split_once(',') {
        Some((n, f)) => (n.trim(), fold(f.trim())),
        None => (q, String::new()),
    };
    if name.chars().count() < 2 {
        return Ok(Vec::new());
    }
    // Ask for many results: local places may rank low globally but go first here.
    let count = 100;
    let url = format!("https://geocoding-api.open-meteo.com/v1/search?count={count}&format=json&language={}&name={}", urlencode(lang), urlencode(name));
    let v = get_json(&url)?;
    let mut places: Vec<Place> = v["results"]
        .as_array()
        .map(|rs| {
            rs.iter()
                .map(|r| {
                    let parts: Vec<&str> = [r["admin1"].as_str(), r["country"].as_str()].into_iter().flatten().filter(|s| !s.is_empty()).collect();
                    Place {
                        name: r["name"].as_str().unwrap_or_default().to_owned(),
                        detail: parts.join(", "),
                        country_code: r["country_code"].as_str().unwrap_or_default().to_owned(),
                        lat: r["latitude"].as_f64().unwrap_or(0.0),
                        lon: r["longitude"].as_f64().unwrap_or(0.0),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    if !filter.is_empty() {
        places.retain(|p| {
            let hay = fold(&format!("{} {}", p.detail, p.country_code));
            filter.split_whitespace().all(|w| hay.contains(w))
        });
    }
    // Stable sort keeps the geocoder's relevance order within each group.
    let home = home_country.to_uppercase();
    places.sort_by_key(|p| p.country_code != home);
    places.truncate(8);
    Ok(places)
}

/// Where the user is: Windows location (GPS/Wi-Fi) when allowed, else IP.
pub fn detect_place(lang: &str) -> anyhow::Result<Place> {
    if let Some((lat, lon)) = windows_location() {
        let name = reverse_name(lat, lon, lang).unwrap_or_default();
        let (name, detail) = name.split_once(", ").map(|(a, b)| (a.to_owned(), b.to_owned())).unwrap_or((name, String::new()));
        return Ok(Place { name: if name.is_empty() { "Current location".into() } else { name }, detail, country_code: String::new(), lat, lon });
    }
    let v = get_json("https://ipinfo.io/json")?;
    let (lat, lon) = v["loc"].as_str().and_then(|s| s.split_once(',')).ok_or_else(|| anyhow::anyhow!("no IP location"))?;
    let detail: Vec<&str> = [v["region"].as_str(), v["country"].as_str()].into_iter().flatten().collect();
    Ok(Place {
        name: v["city"].as_str().unwrap_or("Current location").to_owned(),
        detail: detail.join(", "),
        country_code: v["country"].as_str().unwrap_or_default().to_owned(),
        lat: lat.parse()?,
        lon: lon.parse()?,
    })
}

fn windows_location() -> Option<(f64, f64)> {
    use windows::Devices::Geolocation::{GeolocationAccessStatus, Geolocator};
    let status = Geolocator::RequestAccessAsync().ok()?.join().ok()?;
    if status != GeolocationAccessStatus::Allowed {
        log::info!("Windows location not allowed ({status:?}); using IP location");
        return None;
    }
    let pos = Geolocator::new().ok()?.GetGeopositionAsync().ok()?.join().ok()?;
    let p = pos.Coordinate().ok()?.Point().ok()?.Position().ok()?;
    Some((p.Latitude, p.Longitude))
}

/// "City, Region, Country" for coordinates (BigDataCloud's free client endpoint).
fn reverse_name(lat: f64, lon: f64, lang: &str) -> Option<String> {
    let url = format!("https://api.bigdatacloud.net/data/reverse-geocode-client?latitude={lat}&longitude={lon}&localityLanguage={}", urlencode(lang));
    let v = get_json(&url).ok()?;
    let parts: Vec<&str> = [v["city"].as_str().or(v["locality"].as_str()), v["principalSubdivision"].as_str(), v["countryName"].as_str()]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}
