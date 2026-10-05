//! Offline-Ortsauflösung: nächste Stadt über 15.000 Einwohner aus eingebetteten GeoNames-Daten
//! (`assets/geo/cities15000.tsv`, CC BY 4.0, siehe `assets/geo/LICENSE`). Kein Netzwerk.

use std::sync::LazyLock;

/// Städte innerhalb dieser Entfernung liefern Stadt und Land.
pub const CITY_RADIUS_KM: f64 = 50.0;
/// Bis zu dieser Entfernung zur nächsten Stadt gilt wenigstens das Land.
pub const COUNTRY_RADIUS_KM: f64 = 300.0;

const DATA: &str = include_str!("../../assets/geo/cities15000.tsv");
const EARTH_RADIUS_KM: f64 = 6371.0;
/// Stadtteile großer Städte stehen in GeoNames als eigene Orte (z. B. „São Jorge de Arroios“
/// in Lissabon): Unter den Orten, die höchstens so viel weiter weg liegen wie der nächste,
/// gewinnt der einwohnerstärkste.
const MERGE_KM: f64 = 10.0;
/// Grad Breite je Kilometer (Näherung für den Vorfilter).
const DEG_PER_KM: f64 = 1.0 / 111.0;

struct City {
    name: &'static str,
    country: &'static str,
    lat: f64,
    lon: f64,
    population: u64,
}

/// Ergebnis der Auflösung. `city` fehlt, wenn nur das Land bestimmbar ist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub city: Option<String>,
    pub country: String,
}

static CITIES: LazyLock<Vec<City>> = LazyLock::new(|| {
    let mut cities: Vec<City> = DATA
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            Some(City {
                name: parts.next()?,
                country: parts.next()?,
                lat: parts.next()?.parse().ok()?,
                lon: parts.next()?.parse().ok()?,
                population: parts.next()?.parse().ok()?,
            })
        })
        .collect();
    cities.sort_by(|a, b| a.lat.total_cmp(&b.lat));
    cities
});

fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dphi = (lat2 - lat1).to_radians();
    let dlambda = (lon2 - lon1).to_radians();
    let a = (dphi / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dlambda / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_KM * a.sqrt().asin()
}

/// Nächste Stadt zu `(lat, lon)`. `None` bei ungültigen Koordinaten oder wenn keine Stadt
/// innerhalb von [`COUNTRY_RADIUS_KM`] liegt (offene See, Polargebiet).
pub fn nearest(lat: f64, lon: f64) -> Option<Place> {
    if !lat.is_finite()
        || !lon.is_finite()
        || !(-90.0..=90.0).contains(&lat)
        || !(-180.0..=180.0).contains(&lon)
    {
        return None;
    }
    let cities = &*CITIES;
    let band = COUNTRY_RADIUS_KM * DEG_PER_KM;
    let start = cities.partition_point(|c| c.lat < lat - band);
    let nearby: Vec<(&City, f64)> = cities[start..]
        .iter()
        .take_while(|c| c.lat <= lat + band)
        .map(|c| (c, haversine_km(lat, lon, c.lat, c.lon)))
        .collect();
    let (_, nearest_distance) = nearby.iter().copied().min_by(|a, b| a.1.total_cmp(&b.1))?;
    let (city, distance) = nearby
        .iter()
        .copied()
        .filter(|(_, d)| {
            *d <= nearest_distance + MERGE_KM && *d <= CITY_RADIUS_KM.max(nearest_distance)
        })
        .max_by_key(|(c, _)| c.population)?;
    if distance <= CITY_RADIUS_KM {
        Some(Place {
            city: Some(city.name.to_string()),
            country: city.country.to_string(),
        })
    } else if distance <= COUNTRY_RADIUS_KM {
        Some(Place {
            city: None,
            country: city.country.to_string(),
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daten_sind_eingebettet() {
        assert!(CITIES.len() > 30_000);
    }

    #[test]
    fn lissabon_und_berlin() {
        let lisbon = nearest(38.7223, -9.1393).unwrap();
        assert_eq!(lisbon.country, "PT");
        assert_eq!(lisbon.city.as_deref(), Some("Lisbon"));
        let berlin = nearest(52.5200, 13.4050).unwrap();
        assert_eq!(
            (berlin.country.as_str(), berlin.city.as_deref()),
            ("DE", Some("Berlin"))
        );
    }

    #[test]
    fn vorort_gehoert_zur_naechsten_stadt() {
        // Etwa 20 km östlich von München
        let p = nearest(48.14, 11.9).unwrap();
        assert_eq!(p.country, "DE");
        assert!(p.city.is_some());
    }

    #[test]
    fn offene_see_hat_keinen_ort() {
        assert_eq!(nearest(0.0, -30.0), None);
        assert_eq!(nearest(-45.0, -120.0), None);
    }

    #[test]
    fn fern_der_stadt_nur_das_land() {
        // Mitten in den Alpen, 50 bis 300 km von Städten entfernt gibt es nur das Land, aber
        // nie eine falsche Stadt: Wenn eine Stadt gemeldet wird, ist sie höchstens 50 km weg.
        for (lat, lon) in [(46.55, 10.1), (64.5, -18.0), (-25.0, 134.0)] {
            if let Some(p) = nearest(lat, lon) {
                if let Some(city) = &p.city {
                    let c = CITIES
                        .iter()
                        .find(|c| c.name == city && c.country == p.country)
                        .unwrap();
                    assert!(haversine_km(lat, lon, c.lat, c.lon) <= CITY_RADIUS_KM + 1e-6);
                }
            }
        }
        // Zentralaustralien: weit über 50 km bis zur nächsten Stadt, aber unter 300 km: nur Land.
        let outback = nearest(-25.0, 134.0);
        assert!(outback.is_none_or(|p| p.city.is_none()));
    }

    #[test]
    fn ungueltige_koordinaten() {
        assert_eq!(nearest(f64::NAN, 0.0), None);
        assert_eq!(nearest(91.0, 0.0), None);
        assert_eq!(nearest(0.0, 181.0), None);
    }

    #[test]
    fn haversine_bekannte_strecke() {
        // Berlin - Paris etwa 878 km
        let d = haversine_km(52.52, 13.405, 48.8566, 2.3522);
        assert!((d - 878.0).abs() < 10.0, "{d}");
    }
}
