//! Shared output types. Owned by the core-read work (flights).

use schemars::JsonSchema;
use serde::Serialize;

use crate::time::Stamp;

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AirportRef {
    pub iata: Option<String>,
    pub icao: Option<String>,
    pub name: Option<String>,
    pub city: Option<String>,
    pub country: Option<String>,
    pub tz: Option<String>,
}

/// How the flight relates to the owner.
#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    /// The owner flies it.
    Mine,
    /// The owner follows it (not a passenger).
    Following,
    /// A connected friend's flight.
    Friend,
}

#[derive(Debug, Clone, Serialize, JsonSchema, Default)]
pub struct Ticket {
    pub booking_reference: Option<String>,
    pub seat: Option<String>,
    pub cabin: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Flight {
    /// Flighty flight UUID (Flight.id or ManualFlight.id).
    pub id: String,
    /// Entered by hand in the app (ManualFlight).
    pub manual: bool,
    /// "BA286"
    pub flight_code: String,
    pub airline_iata: Option<String>,
    pub airline_name: Option<String>,
    /// "292"
    pub number: String,
    pub from: AirportRef,
    pub to: AirportRef,
    pub departure_scheduled: Option<Stamp>,
    pub departure_estimated: Option<Stamp>,
    pub departure_actual: Option<Stamp>,
    pub arrival_scheduled: Option<Stamp>,
    pub arrival_estimated: Option<Stamp>,
    pub arrival_actual: Option<Stamp>,
    pub departure_terminal: Option<String>,
    pub departure_gate: Option<String>,
    pub arrival_terminal: Option<String>,
    pub arrival_gate: Option<String>,
    pub baggage_belt: Option<String>,
    pub cancelled: bool,
    pub diverted: bool,
    /// Where a diverted flight actually landed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diverted_to: Option<AirportRef>,
    /// Wheels-up / wheels-down, when Flighty has runway times.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub takeoff_actual: Option<Stamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub landing_actual: Option<Stamp>,
    pub aircraft: Option<String>,
    pub tail_number: Option<String>,
    pub duration_minutes: Option<i64>,
    pub relation: Relation,
    /// Friend's display name when relation = friend.
    pub friend_name: Option<String>,
    pub archived: bool,
    pub ticket: Option<Ticket>,
}
