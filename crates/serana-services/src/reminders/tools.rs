//! What the model may do to a reminder, and the arguments it must supply.
//!
//! One entry point — "/reminder <anything>" — reaches all four actions, so the model picks
//! the verb rather than the user picking a command. That only works because the prompt
//! carries the user's existing reminders: an id the model did not read there is an id it
//! invented, and [`super::ReminderService`] refuses it.

use schemars::JsonSchema;
use serana_domain::tool::ToolSpec;
use serde::Deserialize;

use super::parse::ParsedReminder;

pub(crate) const CREATE: &str = "create_reminder";
pub(crate) const UPDATE: &str = "update_reminder";
pub(crate) const DELETE: &str = "delete_reminder";
pub(crate) const ACKNOWLEDGE: &str = "acknowledge_reminder";
pub(crate) const COMPLETE: &str = "complete_items";

/// Changing a reminder means restating its whole schedule, not patching one field.
///
/// The model has the current schedule in front of it, so "move it to 22:30" is something it
/// can resolve into a complete answer. A partial update would need us to merge, and a merge
/// of a schedule the model only half-described is how a reminder ends up firing on a day
/// nobody asked for.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct UpdateArgs {
    /// The id of the reminder to change, copied exactly from the list of existing
    /// reminders. Never invent one.
    pub id: String,
    /// The complete new schedule and text, including the parts that are not changing.
    #[serde(flatten)]
    pub schedule: ParsedReminder,
}

/// Ticking lines off a reminder's checklist.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct CompleteArgs {
    /// The id of the reminder whose checklist this is, copied exactly from the context.
    pub id: String,
    /// The lines the person has finished, copied from the checklist as closely as you can.
    pub items: Vec<String>,
}

/// Arguments for the two actions that only need to name a reminder.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct TargetArgs {
    /// The id of the reminder, copied exactly from the list of existing reminders. Never
    /// invent one.
    pub id: String,
}

/// Every tool offered on a reminder turn.
pub(crate) fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec::typed::<ParsedReminder>(
            CREATE,
            "Create a new reminder from a schedule the person described. Use this when they \
             are asking for something new rather than referring to a reminder that already \
             exists.",
        ),
        ToolSpec::typed::<UpdateArgs>(
            UPDATE,
            "Change an existing reminder's schedule or text. Give the complete new schedule, \
             including the parts that stay the same.",
        ),
        ToolSpec::typed::<TargetArgs>(
            DELETE,
            "Delete an existing reminder for good. Only call this when the person clearly \
             means to remove it, and only with an id you were given.",
        ),
        ToolSpec::typed::<CompleteArgs>(
            COMPLETE,
            "Tick items off a reminder's checklist. Use this when the person says they have \
             finished some of the things on it, not all of it — when the last item is \
             ticked the reminder falls quiet for the period on its own.",
        ),
        ToolSpec::typed::<TargetArgs>(
            ACKNOWLEDGE,
            "Mark an existing reminder as dealt with for the current period. It stops firing \
             until the next period rather than being deleted — use this when the person says \
             they have done the thing, not that they want the reminder gone.",
        ),
    ]
}

/// Whether `name` is one of ours, so a hallucinated tool name is not dispatched.
pub(crate) fn is_known(name: &str) -> bool {
    matches!(name, CREATE | UPDATE | DELETE | ACKNOWLEDGE | COMPLETE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_is_offered_and_recognised() {
        let specs = specs();
        assert_eq!(specs.len(), 5);
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
        for name in ["", "delete_everything", "create", "reminder"] {
            assert!(!is_known(name), "{name:?} should not be dispatched");
        }
    }

    #[test]
    fn updating_asks_for_an_id_alongside_a_whole_schedule() {
        // Flattened, so the model fills one flat object rather than a nested one — the
        // same reason `ParsedReminder` is flat in the first place.
        let spec = specs()
            .into_iter()
            .find(|spec| spec.name == UPDATE)
            .unwrap();
        let properties = spec.parameters["properties"].as_object().unwrap();
        for field in ["id", "kind", "time", "text", "items"] {
            assert!(
                properties.contains_key(field),
                "{field} missing from {properties:?}"
            );
        }
    }

    #[test]
    fn the_targeting_tools_ask_only_for_an_id() {
        for name in [DELETE, ACKNOWLEDGE] {
            let spec = specs().into_iter().find(|spec| spec.name == name).unwrap();
            let properties = spec.parameters["properties"].as_object().unwrap();
            assert_eq!(properties.len(), 1, "{name}");
            assert!(properties.contains_key("id"), "{name}");
        }
    }
}
