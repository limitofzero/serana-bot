//! Reminder lifecycle: creating, changing, finishing and cancelling.
//!
//! No model, no conversation. Everything here is a decision that can be made from stored
//! state and a clock, which is what makes it testable without scripting a provider — and
//! what keeps "what does a reminder do" separate from "how was it asked for".

use serana_domain::reminder::{
    Recurrence, Reminder, ReminderId, ReminderRepository, TimeZoneName, TodoItem, UserId,
};
use serana_domain::{Clock, IdGenerator};

use super::error::ReminderError;

/// Reminder lifecycle, independent of how the request arrived.
pub struct ReminderService<R, C, I> {
    pub(super) repository: R,
    pub(super) clock: C,
    ids: I,
}

impl<R, C, I> ReminderService<R, C, I>
where
    R: ReminderRepository,
    C: Clock,
    I: IdGenerator,
{
    pub fn new(repository: R, clock: C, ids: I) -> Self {
        Self {
            repository,
            clock,
            ids,
        }
    }

    pub fn now(&self) -> jiff::Timestamp {
        self.clock.now()
    }

    /// What `owner` still has coming, soonest first.
    ///
    /// Spent reminders are left out. A one-off that has already fired, or a reminder whose
    /// schedule has run out, is history — it can never fire again, so listing it only
    /// crowds out the ones that still matter. They stay in storage, and the model still
    /// sees them, so "delete the one from yesterday" keeps working.
    pub async fn create(
        &self,
        owner: UserId,
        now: jiff::Timestamp,
        timezone: TimeZoneName,
        recurrence: Recurrence,
        text: String,
        items: Vec<TodoItem>,
    ) -> Result<Reminder, ReminderError> {
        let mut reminder = Reminder {
            id: ReminderId::new(self.ids.generate()),
            owner,
            text,
            items,
            recurrence,
            timezone,
            created_at: now,
            next_fire_at: None,
            last_fired_at: None,
            acknowledged_through: None,
        };

        // Refuse before storing rather than after: a reminder that can never fire is
        // clutter the user would have to find and delete.
        if reminder.reschedule(now)?.is_none() {
            return Err(ReminderError::NeverFires);
        }

        self.repository.put(&reminder).await?;
        Ok(reminder)
    }

    /// Replace one of `owner`'s reminders' schedule and text, keeping its identity.
    ///
    /// `created_at` and `last_fired_at` survive because it is the same reminder. Any
    /// outstanding acknowledgement does not: the new schedule may divide time into
    /// different periods, and a watermark from the old one could silence days the user
    /// has just asked for. One redundant reminder beats a silently swallowed one.
    pub async fn update(
        &self,
        owner: UserId,
        id: &ReminderId,
        recurrence: Recurrence,
        text: String,
        items: Vec<TodoItem>,
        now: jiff::Timestamp,
    ) -> Result<Reminder, ReminderError> {
        let mut reminder = self
            .repository
            .get(id)
            .await?
            .filter(|reminder| reminder.owner == owner)
            .ok_or_else(|| ReminderError::NotFound(id.clone()))?;

        // A reminder that was already spent cannot be made worse by an edit. Refusing one
        // would freeze it: a one-off whose moment has passed could never be given a
        // checklist, renamed, or turned into a repeating reminder — only deleted.
        let was_spent = !reminder.is_active();

        reminder.recurrence = recurrence;
        reminder.text = text;
        reminder.relist(items);
        reminder.acknowledged_through = None;
        if reminder.reschedule(now)?.is_none() && !was_spent {
            return Err(ReminderError::NeverFires);
        }

        // Clearing the acknowledgement above is the safe default for a changed schedule,
        // but it must not resurrect a checklist that is still finished — editing a typo
        // should not start the nagging again.
        if reminder.all_done(now)? {
            reminder.acknowledge(now)?;
        }

        self.repository.put(&reminder).await?;
        Ok(reminder)
    }

    /// The moment this service reckons by.
    ///
    /// Exposed so a caller rendering a checklist decides what is ticked against the same
    /// clock the service does, rather than reaching for the system one.
    pub async fn list(&self, owner: UserId) -> Result<Vec<Reminder>, ReminderError> {
        Ok(self
            .repository
            .list_for_owner(owner)
            .await?
            .into_iter()
            .filter(Reminder::is_active)
            .collect())
    }

    /// Everything `owner` has, spent ones included.
    pub async fn list_all(&self, owner: UserId) -> Result<Vec<Reminder>, ReminderError> {
        Ok(self.repository.list_for_owner(owner).await?)
    }

    /// Mark this period dealt with: stop firing until the next one comes round.
    ///
    /// The reminder is not deleted — a monthly invoice reminder acknowledged in March is
    /// silent for the rest of March and back in April. A [`Recurrence::Once`] has no next
    /// period, so acknowledging one retires it.
    pub async fn acknowledge(
        &self,
        owner: UserId,
        id: &ReminderId,
    ) -> Result<Reminder, ReminderError> {
        let mut reminder = self
            .repository
            .get(id)
            .await?
            .filter(|reminder| reminder.owner == owner)
            .ok_or_else(|| ReminderError::NotFound(id.clone()))?;

        self.retire(&mut reminder, self.clock.now()).await?;
        Ok(reminder)
    }

    /// The reminder's work for this period is finished. Put it away accordingly.
    ///
    /// A repeating reminder falls quiet until its next period and stays on the list — it
    /// has more to do later. A one-off has nothing left to come back for, so keeping it
    /// would leave a row that can never fire again cluttering every listing. It is deleted.
    async fn retire(
        &self,
        reminder: &mut Reminder,
        now: jiff::Timestamp,
    ) -> Result<(), ReminderError> {
        if reminder.recurrence.is_recurring() {
            reminder.acknowledge(now)?;
            self.repository.put(reminder).await?;
        } else {
            reminder.next_fire_at = None;
            self.repository.delete(&reminder.id).await?;
        }
        Ok(())
    }

    /// Tick items off a reminder's checklist.
    ///
    /// Ticking the last outstanding item acknowledges the period: the job is done, so the
    /// remaining days of the window have nothing left to nag about. That is the same thing
    /// [`Self::acknowledge`] does, reached by finishing the work rather than by saying so.
    pub async fn complete_items(
        &self,
        owner: UserId,
        id: &ReminderId,
        items: &[String],
        now: jiff::Timestamp,
    ) -> Result<(Reminder, Vec<String>), ReminderError> {
        let mut reminder = self
            .repository
            .get(id)
            .await?
            .filter(|reminder| reminder.owner == owner)
            .ok_or_else(|| ReminderError::NotFound(id.clone()))?;

        if reminder.items.is_empty() {
            return Err(ReminderError::NoChecklist(id.clone()));
        }

        let ticked = reminder.complete(items, now);
        if reminder.all_done(now)? {
            self.retire(&mut reminder, now).await?;
        } else {
            self.repository.put(&reminder).await?;
        }
        Ok((reminder, ticked))
    }

    /// Cancel one of `owner`'s reminders.
    ///
    /// A reminder belonging to somebody else reports [`ReminderError::NotFound`] rather
    /// than a permission error, so ids cannot be probed for existence.
    pub async fn delete(&self, owner: UserId, id: &ReminderId) -> Result<Reminder, ReminderError> {
        let reminder = self
            .repository
            .get(id)
            .await?
            .filter(|reminder| reminder.owner == owner)
            .ok_or_else(|| ReminderError::NotFound(id.clone()))?;

        self.repository.delete(id).await?;
        Ok(reminder)
    }
}
