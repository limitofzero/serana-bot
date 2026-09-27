//! What the model may do about prices, and the arguments it must supply.
//!
//! One entry point — "/prices <anything>" — reaches all of it, so the model picks the verb
//! rather than the person picking a command. There is nothing to identify: a person has one
//! digest, so no tool here takes an id.

use schemars::JsonSchema;
use serana_domain::tool::ToolSpec;
use serde::Deserialize;

use crate::schedule::ParsedSchedule;

pub(crate) const SHOW: &str = "show_prices";
pub(crate) const SCHEDULE: &str = "set_schedule";
pub(crate) const PAUSE: &str = "pause_digest";
pub(crate) const RESUME: &str = "resume_digest";

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct ShowArgs {
    /// Whether to read the market again rather than showing what was read this morning.
    ///
    /// False for an ordinary "what are prices?" — the daily digest is already stored and
    /// answering from it is instant. True only when they ask for something current:
    /// "refresh", "right now", "latest", "check again".
    pub fresh: bool,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct ScheduleArgs {
    /// When the digest should arrive from now on. Give the complete schedule, including
    /// the parts that are not changing.
    #[serde(flatten)]
    pub schedule: ParsedSchedule,
    /// An IANA time zone such as `Europe/Lisbon`, if they are moving the digest to a
    /// different one. Leave it out to keep the zone it already has — which is almost
    /// always right, because "at 8" means 8 where they are.
    #[serde(default)]
    pub timezone: Option<String>,
}

/// Neither switching off nor back on needs an argument. An empty object is what the schema
/// has to say so.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct NoArgs {}

/// Every tool offered on a price turn.
pub(crate) fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec::typed::<ShowArgs>(
            SHOW,
            "Show the prices. Use this whenever they ask what something is worth, how the \
             market is, or for their digest.",
        ),
        ToolSpec::typed::<ScheduleArgs>(
            SCHEDULE,
            "Change when the daily digest arrives. Use this when they say when they want \
             it — \"at 8\", \"weekday mornings\", \"only on the 1st\" — and give the whole \
             schedule, not just the part that changed.",
        ),
        ToolSpec::typed::<NoArgs>(
            PAUSE,
            "Stop the digest arriving. Asking still works and still answers instantly; \
             nothing arrives unprompted. Use this for \"stop sending it\", \"mute it\", \
             \"turn it off\".",
        ),
        ToolSpec::typed::<NoArgs>(
            RESUME,
            "Start the digest arriving again, on the schedule it already has.",
        ),
    ]
}

/// Whether `name` is one of ours, so a hallucinated tool name is not dispatched.
pub(crate) fn is_known(name: &str) -> bool {
    matches!(name, SHOW | SCHEDULE | PAUSE | RESUME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_is_offered_and_recognised() {
        let specs = specs();
        assert_eq!(specs.len(), 4);
        for spec in &specs {
            assert!(
                is_known(&spec.name),
                "{} is offered but not dispatched",
                spec.name
            );
            assert!(!spec.description.trim().is_empty(), "{}", spec.name);
        }
    }

    #[test]
    fn a_tool_name_we_do_not_offer_is_not_dispatched() {
        for name in ["", "sell_everything", "prices", "show"] {
            assert!(!is_known(name), "{name:?} should not be dispatched");
        }
    }

    #[test]
    fn rescheduling_asks_for_a_whole_schedule_beside_the_zone() {
        // Flattened, so the model fills one flat object — the same reason the shared
        // schedule is flat in the first place.
        let spec = specs().into_iter().find(|s| s.name == SCHEDULE).unwrap();
        let properties = spec.parameters["properties"].as_object().unwrap();
        for field in ["kind", "time", "weekdays", "days_of_month", "timezone"] {
            assert!(
                properties.contains_key(field),
                "{field} missing from {properties:?}"
            );
        }
    }

    #[test]
    fn showing_asks_only_whether_to_read_the_market_again() {
        let spec = specs().into_iter().find(|s| s.name == SHOW).unwrap();
        let properties = spec.parameters["properties"].as_object().unwrap();
        assert_eq!(properties.len(), 1, "{properties:?}");
        assert!(properties.contains_key("fresh"));
    }

    #[test]
    fn switching_off_takes_no_arguments() {
        // A tool with invented arguments is a tool a model fills in with invented values.
        for name in [PAUSE, RESUME] {
            let spec = specs().into_iter().find(|s| s.name == name).unwrap();
            let properties = spec.parameters["properties"].as_object();
            assert!(
                properties.is_none_or(|p| p.is_empty()),
                "{name}: {:?}",
                spec.parameters
            );
        }
    }
}
