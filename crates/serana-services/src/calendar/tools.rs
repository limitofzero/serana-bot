//! What the model may do to the calendar, and the arguments it must supply.
//!
//! One entry point — "/calendar <anything>" — reaches all four actions, so the model picks
//! the verb rather than the person picking a command. That only works because the prompt
//! carries the week ahead: an id the model did not read there, or in a listing it has just
//! made, is an id it invented, and [`super::CalendarService`] refuses it.

use schemars::JsonSchema;
use serana_domain::tool::ToolSpec;
use serde::Deserialize;

pub(crate) const LIST: &str = "list_events";
pub(crate) const CREATE: &str = "create_event";
pub(crate) const CANCEL: &str = "cancel_event";
pub(crate) const DELETE: &str = "delete_event";

/// A span of days to look at.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct RangeArgs {
    /// First day, as `YYYY-MM-DD`. Resolve "tomorrow", "next week" and the like against the
    /// date in the context line.
    pub from: String,
    /// Last day, as `YYYY-MM-DD`. Included in the answer, so a single day has the same
    /// value here as in `from`.
    pub to: String,
}

/// An appointment to put on the calendar.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct NewEventArgs {
    /// A short title, in the person's own language and their own words. Not a sentence:
    /// "dentist", not "an appointment with the dentist".
    pub summary: String,
    /// The day, as `YYYY-MM-DD`.
    pub date: String,
    /// When it starts, on a 24-hour clock, as `HH:MM`. Local time — the same clock as the
    /// context line.
    pub start_time: String,
    /// When it ends, as `HH:MM`. Leave it out if they did not say; an hour is assumed.
    #[serde(default)]
    pub end_time: Option<String>,
    /// Where it is, if they said. An address or a place name, as they wrote it.
    #[serde(default)]
    pub location: Option<String>,
    /// Whether they have to physically go somewhere: a clinic, an office, a café, someone's
    /// home. True for those. False for a call, a video meeting, or anything at home.
    ///
    /// When it is true the calendar also reserves the road either side, so do NOT widen the
    /// times yourself to allow for travel — give the appointment's own hours.
    pub in_person: bool,
}

/// Arguments for the two actions that only need to name an event.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct TargetArgs {
    /// The id of the event, copied exactly from the context line or from a listing you have
    /// just made. Never invent one.
    pub id: String,
}

/// Every tool offered on a calendar turn.
pub(crate) fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec::typed::<RangeArgs>(
            LIST,
            "Show what is on between two days. Use this whenever they ask what they have \
             on, even for a day already in the context line — the answer is written out for \
             them from the calendar itself rather than retold.",
        ),
        ToolSpec::typed::<NewEventArgs>(
            CREATE,
            "Put a new appointment on the calendar. Use this when they are arranging \
             something, not when they are asking about something already there.",
        ),
        ToolSpec::typed::<TargetArgs>(
            CANCEL,
            "Take an event off the day without destroying it: their own is struck off, an \
             invitation from somebody else is declined and stays on the calendar marked as \
             not attending. This is what \"cancel\", \"I am not going\" and \"call it off\" \
             mean. For one occurrence of a repeating event it affects only that occurrence.",
        ),
        ToolSpec::typed::<TargetArgs>(
            DELETE,
            "Delete an event for good. Irreversible, and almost never what they mean — \
             prefer cancel_event. Only call this when they have said, in an earlier message, \
             that they want it deleted permanently after you asked them to confirm.",
        ),
    ]
}

/// Whether `name` is one of ours, so a hallucinated tool name is not dispatched.
pub(crate) fn is_known(name: &str) -> bool {
    matches!(name, LIST | CREATE | CANCEL | DELETE)
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
        for name in ["", "drop_calendar", "create", "event"] {
            assert!(!is_known(name), "{name:?} should not be dispatched");
        }
    }

    #[test]
    fn creating_asks_for_everything_needed_to_place_a_block() {
        let spec = specs().into_iter().find(|s| s.name == CREATE).unwrap();
        let properties = spec.parameters["properties"].as_object().unwrap();
        for field in ["summary", "date", "start_time", "in_person"] {
            assert!(
                properties.contains_key(field),
                "{field} missing from {properties:?}"
            );
        }
        // What the person did not say must not be invented: these are the two the model is
        // allowed to leave out.
        let required: Vec<&str> = spec.parameters["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(!required.contains(&"end_time"), "{required:?}");
        assert!(!required.contains(&"location"), "{required:?}");
    }

    #[test]
    fn the_targeting_tools_ask_only_for_an_id() {
        for name in [CANCEL, DELETE] {
            let spec = specs().into_iter().find(|s| s.name == name).unwrap();
            let properties = spec.parameters["properties"].as_object().unwrap();
            assert_eq!(properties.len(), 1, "{name}");
            assert!(properties.contains_key("id"), "{name}");
        }
    }

    #[test]
    fn deleting_is_described_as_the_last_resort_it_is() {
        // The one irreversible action here. If its description reads like cancelling, the
        // model will reach for it when somebody simply is not going.
        let spec = specs().into_iter().find(|s| s.name == DELETE).unwrap();
        assert!(spec.description.contains("Irreversible"), "{spec:?}");
        assert!(spec.description.contains("confirm"), "{spec:?}");
    }
}
