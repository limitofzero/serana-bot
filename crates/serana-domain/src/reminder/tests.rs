//! Behaviour of reminders, recurrence and checklists.

use jiff::civil::{date, time};

use crate::error::StorageError;

use super::*;

/// Georgia has no daylight saving, so this zone isolates recurrence logic from DST.
fn tbilisi() -> jiff::tz::TimeZone {
    jiff::tz::TimeZone::get("Asia/Tbilisi").unwrap()
}

/// Has DST, so transitions are exercised even though the default zone does not.
fn berlin() -> jiff::tz::TimeZone {
    jiff::tz::TimeZone::get("Europe/Berlin").unwrap()
}

fn at(y: i16, m: i8, d: i8, h: i8, min: i8, tz: &jiff::tz::TimeZone) -> jiff::Timestamp {
    date(y, m, d)
        .at(h, min, 0, 0)
        .to_zoned(tz.clone())
        .unwrap()
        .timestamp()
}

fn local(ts: jiff::Timestamp, tz: &jiff::tz::TimeZone) -> jiff::civil::DateTime {
    ts.to_zoned(tz.clone()).datetime()
}

#[test]
fn a_daily_reminder_fires_later_today_when_the_time_has_not_passed() {
    let tz = tbilisi();
    let r = Recurrence::Daily {
        at: time(10, 0, 0, 0),
    };
    let next = r
        .next_occurrence_after(at(2026, 3, 10, 8, 0, &tz), &tz)
        .unwrap();
    assert_eq!(local(next, &tz), date(2026, 3, 10).at(10, 0, 0, 0));
}

#[test]
fn a_daily_reminder_rolls_to_tomorrow_once_the_time_has_passed() {
    let tz = tbilisi();
    let r = Recurrence::Daily {
        at: time(10, 0, 0, 0),
    };
    let next = r
        .next_occurrence_after(at(2026, 3, 10, 11, 0, &tz), &tz)
        .unwrap();
    assert_eq!(local(next, &tz), date(2026, 3, 11).at(10, 0, 0, 0));
}

#[test]
fn the_exact_firing_instant_yields_the_next_one_not_the_same_one() {
    // Otherwise a scheduler that wakes precisely on time reschedules to now, forever.
    let tz = tbilisi();
    let r = Recurrence::Daily {
        at: time(10, 0, 0, 0),
    };
    let now = at(2026, 3, 10, 10, 0, &tz);
    let next = r.next_occurrence_after(now, &tz).unwrap();
    assert!(next > now);
    assert_eq!(local(next, &tz), date(2026, 3, 11).at(10, 0, 0, 0));
}

#[test]
fn a_weekly_reminder_finds_the_next_matching_weekday() {
    let tz = tbilisi();
    // 2026-03-10 is a Tuesday.
    let r = Recurrence::Weekly {
        days: WeekDays::new([Weekday::Friday]).unwrap(),
        at: time(9, 30, 0, 0),
    };
    let next = r
        .next_occurrence_after(at(2026, 3, 10, 12, 0, &tz), &tz)
        .unwrap();
    assert_eq!(local(next, &tz), date(2026, 3, 13).at(9, 30, 0, 0));
}

#[test]
fn a_weekly_reminder_on_today_still_fires_today_if_the_time_is_ahead() {
    let tz = tbilisi();
    let r = Recurrence::Weekly {
        days: WeekDays::new([Weekday::Tuesday]).unwrap(),
        at: time(18, 0, 0, 0),
    };
    let next = r
        .next_occurrence_after(at(2026, 3, 10, 12, 0, &tz), &tz)
        .unwrap();
    assert_eq!(local(next, &tz), date(2026, 3, 10).at(18, 0, 0, 0));
}

#[test]
fn a_weekly_reminder_on_today_rolls_a_full_week_once_the_time_has_passed() {
    let tz = tbilisi();
    let r = Recurrence::Weekly {
        days: WeekDays::new([Weekday::Tuesday]).unwrap(),
        at: time(9, 0, 0, 0),
    };
    let next = r
        .next_occurrence_after(at(2026, 3, 10, 12, 0, &tz), &tz)
        .unwrap();
    assert_eq!(local(next, &tz), date(2026, 3, 17).at(9, 0, 0, 0));
}

#[test]
fn the_invoice_reminder_from_the_brief_lands_on_the_twentieth() {
    let tz = tbilisi();
    let r = Recurrence::Monthly {
        days: MonthDays::new([20]).unwrap(),
        at: time(10, 0, 0, 0),
    };
    let next = r
        .next_occurrence_after(at(2026, 3, 5, 9, 0, &tz), &tz)
        .unwrap();
    assert_eq!(local(next, &tz), date(2026, 3, 20).at(10, 0, 0, 0));

    // And from just after it, the same day next month.
    let after = r
        .next_occurrence_after(at(2026, 3, 20, 10, 1, &tz), &tz)
        .unwrap();
    assert_eq!(local(after, &tz), date(2026, 4, 20).at(10, 0, 0, 0));
}

#[test]
fn a_monthly_reminder_skips_months_without_that_day_rather_than_clamping() {
    let tz = tbilisi();
    let r = Recurrence::Monthly {
        days: MonthDays::new([31]).unwrap(),
        at: time(10, 0, 0, 0),
    };
    // From 1 February 2026: February has 28 days, so the next is 31 March.
    let next = r
        .next_occurrence_after(at(2026, 2, 1, 0, 0, &tz), &tz)
        .unwrap();
    assert_eq!(local(next, &tz), date(2026, 3, 31).at(10, 0, 0, 0));
}

#[test]
fn a_monthly_reminder_on_the_twenty_ninth_finds_a_leap_day() {
    let tz = tbilisi();
    let r = Recurrence::Monthly {
        days: MonthDays::new([29]).unwrap(),
        at: time(8, 0, 0, 0),
    };
    // 2028 is a leap year.
    let next = r
        .next_occurrence_after(at(2028, 2, 1, 0, 0, &tz), &tz)
        .unwrap();
    assert_eq!(local(next, &tz), date(2028, 2, 29).at(8, 0, 0, 0));
}

#[test]
fn an_impossible_day_of_the_month_cannot_be_constructed() {
    // This used to be a schedule that silently never fired. Refusing it at the
    // constructor means a hallucinated day 45 is reported to the user instead.
    for day in [0, 32, -1, 100] {
        assert_eq!(
            MonthDays::new([day]),
            Err(InvalidDays::OutOfRange(day)),
            "day {day}"
        );
    }
    assert_eq!(MonthDays::new([]), Err(InvalidDays::Empty));
    assert_eq!(WeekDays::new([]), Err(InvalidDays::Empty));
}

/// A reminder nagging daily from the 20th to the 26th, in Tbilisi.
fn nagging() -> Reminder {
    Reminder {
        id: ReminderId::new("r1"),
        owner: UserId::new(1),
        text: "issue the invoice".into(),
        items: Vec::new(),
        recurrence: Recurrence::Monthly {
            days: MonthDays::new([20, 21, 22, 23, 24, 25, 26]).unwrap(),
            at: time(22, 30, 0, 0),
        },
        timezone: TimeZoneName::new("Asia/Tbilisi"),
        created_at: jiff::Timestamp::UNIX_EPOCH,
        next_fire_at: None,
        last_fired_at: None,
        acknowledged_through: None,
    }
}

/// The payment checklist from the brief: days 1-6 each month, three things to do.
fn payment_checklist() -> Reminder {
    let mut r = nagging();
    r.text = "monthly payment".into();
    r.recurrence = Recurrence::Monthly {
        days: MonthDays::new([1, 2, 3, 4, 5, 6]).unwrap(),
        at: time(9, 0, 0, 0),
    };
    r.items = vec![
        TodoItem::new("exchange money"),
        TodoItem::new("transfer to tbc"),
        TodoItem::new("write to the banker"),
    ];
    r
}

#[test]
fn a_tick_counts_this_period_and_goes_stale_in_the_next() {
    // The whole reason `done_at` is a timestamp rather than a flag.
    let tz = tbilisi();
    let mut r = payment_checklist();
    let march = at(2026, 3, 2, 10, 0, &tz);
    r.complete(&["exchange money".into()], march);

    assert!(r.is_done(&r.items[0], march).unwrap(), "done in March");
    assert_eq!(r.outstanding(march).unwrap().len(), 2);

    // April: the same tick no longer counts, and nothing had to reset it.
    let april = at(2026, 4, 1, 10, 0, &tz);
    assert!(!r.is_done(&r.items[0], april).unwrap());
    assert_eq!(r.outstanding(april).unwrap().len(), 3);
}

#[test]
fn a_tick_earlier_in_the_same_period_still_counts() {
    // Day 1 ticked, read back on day 6: the window is the month, not the day.
    let tz = tbilisi();
    let mut r = payment_checklist();
    r.complete(&["exchange money".into()], at(2026, 3, 1, 10, 0, &tz));
    assert!(
        r.is_done(&r.items[0], at(2026, 3, 6, 23, 0, &tz)).unwrap(),
        "still done on the 6th"
    );
}

#[test]
fn matching_is_forgiving_but_refuses_to_guess_between_two_lines() {
    let tz = tbilisi();
    let now = at(2026, 3, 2, 10, 0, &tz);
    let mut r = payment_checklist();

    // Exact, and a paraphrase that contains only one line.
    assert_eq!(
        r.complete(&["EXCHANGE MONEY".into(), "transfer".into()], now),
        vec!["exchange money", "transfer to tbc"]
    );

    // "to" appears in two remaining lines... and in none now, so nothing is ticked.
    let mut fresh = payment_checklist();
    assert!(fresh.complete(&["to".into()], now).is_empty(), "ambiguous");
    assert_eq!(fresh.outstanding(now).unwrap().len(), 3);
}

#[test]
fn a_phrase_matching_nothing_ticks_nothing() {
    let tz = tbilisi();
    let now = at(2026, 3, 2, 10, 0, &tz);
    let mut r = payment_checklist();
    assert!(
        r.complete(&["feed the cat".into(), "  ".into()], now)
            .is_empty()
    );
    assert_eq!(r.outstanding(now).unwrap().len(), 3);
}

#[test]
fn all_done_is_false_for_a_reminder_with_no_checklist() {
    // An empty list is not "finished" — it never had anything to finish.
    let tz = tbilisi();
    assert!(!nagging().all_done(at(2026, 3, 20, 23, 0, &tz)).unwrap());
}

#[test]
fn finishing_the_checklist_is_what_ends_the_period() {
    let tz = tbilisi();
    let mut r = payment_checklist();
    r.reschedule(at(2026, 3, 1, 0, 0, &tz)).unwrap();

    let now = at(2026, 3, 2, 10, 0, &tz);
    r.complete(
        &[
            "exchange money".into(),
            "transfer to tbc".into(),
            "write to the banker".into(),
        ],
        now,
    );
    assert!(r.all_done(now).unwrap());

    // Acknowledging on the strength of that skips days 3-6.
    r.acknowledge(now).unwrap();
    assert_eq!(
        local(r.next_fire_at.unwrap(), &tz),
        date(2026, 4, 1).at(9, 0, 0, 0)
    );
}

#[test]
fn a_period_start_is_the_mirror_of_its_end() {
    let tz = tbilisi();
    let noon = at(2026, 3, 11, 12, 0, &tz);
    let start = |r: &Recurrence| local(r.period_start_of(noon, &tz).unwrap(), &tz).date();

    // 2026-03-11 is a Wednesday, so its week began on Monday the 9th.
    assert_eq!(
        start(&Recurrence::Daily {
            at: time(9, 0, 0, 0)
        }),
        date(2026, 3, 11)
    );
    assert_eq!(
        start(&Recurrence::Weekly {
            days: WeekDays::new([Weekday::Monday]).unwrap(),
            at: time(9, 0, 0, 0)
        }),
        date(2026, 3, 9)
    );
    assert_eq!(start(&payment_checklist().recurrence), date(2026, 3, 1));
    assert_eq!(
        Recurrence::Once {
            at: date(2026, 3, 20).at(9, 0, 0, 0)
        }
        .period_start_of(noon, &tz),
        None,
        "a one-off has no periods, so a tick never goes stale"
    );
}

#[test]
fn acknowledging_silences_the_rest_of_the_period_and_no_further() {
    let tz = tbilisi();
    let mut r = nagging();
    r.reschedule(at(2026, 3, 1, 0, 0, &tz)).unwrap();
    assert_eq!(
        local(r.next_fire_at.unwrap(), &tz),
        date(2026, 3, 20).at(22, 30, 0, 0)
    );

    // The user replies "done" on the 21st, having sent the invoice.
    let next = r.acknowledge(at(2026, 3, 21, 23, 0, &tz)).unwrap().unwrap();
    // The 22nd through the 26th are skipped; April starts clean.
    assert_eq!(local(next, &tz), date(2026, 4, 20).at(22, 30, 0, 0));
}

#[test]
fn an_acknowledgement_expires_with_its_period_rather_than_needing_to_be_cleared() {
    let tz = tbilisi();
    let mut r = nagging();
    r.acknowledge(at(2026, 3, 21, 23, 0, &tz)).unwrap();
    let watermark = r.acknowledged_through.unwrap();

    // Once April is under way the stale watermark has no effect at all.
    r.reschedule(at(2026, 4, 22, 0, 0, &tz)).unwrap();
    assert!(watermark < at(2026, 4, 22, 0, 0, &tz));
    assert_eq!(
        local(r.next_fire_at.unwrap(), &tz),
        date(2026, 4, 22).at(22, 30, 0, 0)
    );
}

#[test]
fn a_delivery_racing_an_acknowledgement_cannot_revive_the_silenced_days() {
    // The scheduler may be mid-tick when the user answers. `mark_fired` must not undo
    // the acknowledgement by rescheduling from `now`.
    let tz = tbilisi();
    let mut r = nagging();
    r.acknowledge(at(2026, 3, 21, 23, 0, &tz)).unwrap();
    r.mark_fired(at(2026, 3, 21, 23, 1, &tz)).unwrap();
    assert_eq!(
        local(r.next_fire_at.unwrap(), &tz),
        date(2026, 4, 20).at(22, 30, 0, 0),
        "the 22nd must stay silenced"
    );
}

#[test]
fn acknowledging_a_one_off_retires_it() {
    let tz = tbilisi();
    let mut r = nagging();
    r.recurrence = Recurrence::Once {
        at: date(2026, 3, 20).at(9, 0, 0, 0),
    };
    r.reschedule(at(2026, 3, 1, 0, 0, &tz)).unwrap();
    assert!(r.is_active());

    assert_eq!(r.acknowledge(at(2026, 3, 20, 9, 5, &tz)).unwrap(), None);
    assert!(!r.is_active(), "a one-off has no next period");
}

#[test]
fn a_period_is_the_calendar_unit_the_recurrence_repeats_over() {
    let tz = tbilisi();
    let noon = at(2026, 3, 11, 12, 0, &tz);
    let end = |r: &Recurrence| local(r.period_end_after(noon, &tz).unwrap(), &tz).date();

    // 2026-03-11 is a Wednesday, so its week ends on Sunday the 15th.
    assert_eq!(
        end(&Recurrence::Daily {
            at: time(9, 0, 0, 0)
        }),
        date(2026, 3, 11)
    );
    assert_eq!(
        end(&Recurrence::Weekly {
            days: WeekDays::new([Weekday::Monday]).unwrap(),
            at: time(9, 0, 0, 0)
        }),
        date(2026, 3, 15)
    );
    assert_eq!(end(&nagging().recurrence), date(2026, 3, 31));
    assert_eq!(
        Recurrence::Once {
            at: date(2026, 3, 20).at(9, 0, 0, 0)
        }
        .period_end_after(noon, &tz),
        None
    );
}

#[test]
fn days_are_sorted_and_deduplicated_so_equal_schedules_compare_equal() {
    let scrambled = MonthDays::new([26, 20, 22, 20, 21]).unwrap();
    assert_eq!(scrambled.iter().collect::<Vec<_>>(), vec![20, 21, 22, 26]);
    assert_eq!(scrambled.len(), 4);
    assert_eq!(MonthDays::new([20, 21]), MonthDays::new([21, 20, 21]));
}

#[test]
fn a_monthly_range_fires_on_every_day_of_the_range() {
    // The request that motivated day sets: nag daily from the 20th to the 26th.
    let tz = tbilisi();
    let r = Recurrence::Monthly {
        days: MonthDays::new([20, 21, 22, 23, 24, 25, 26]).unwrap(),
        at: time(22, 30, 0, 0),
    };
    let mut cursor = at(2026, 3, 1, 0, 0, &tz);
    let mut fired = Vec::new();
    for _ in 0..9 {
        cursor = r.next_occurrence_after(cursor, &tz).unwrap();
        fired.push(local(cursor, &tz));
    }
    // Seven days this month, then it rolls into the next.
    assert_eq!(fired[0], date(2026, 3, 20).at(22, 30, 0, 0));
    assert_eq!(fired[6], date(2026, 3, 26).at(22, 30, 0, 0));
    assert_eq!(fired[7], date(2026, 4, 20).at(22, 30, 0, 0));
    assert_eq!(fired[8], date(2026, 4, 21).at(22, 30, 0, 0));
}

#[test]
fn a_month_without_a_listed_day_skips_it_and_keeps_the_others() {
    // February has no 30th or 31st; the 28th still fires.
    let tz = tbilisi();
    let r = Recurrence::Monthly {
        days: MonthDays::new([28, 30, 31]).unwrap(),
        at: time(9, 0, 0, 0),
    };
    let first = r
        .next_occurrence_after(at(2026, 2, 1, 0, 0, &tz), &tz)
        .unwrap();
    assert_eq!(local(first, &tz), date(2026, 2, 28).at(9, 0, 0, 0));
    let second = r.next_occurrence_after(first, &tz).unwrap();
    assert_eq!(local(second, &tz), date(2026, 3, 28).at(9, 0, 0, 0));
}

#[test]
fn a_weekly_reminder_fires_on_each_listed_weekday() {
    let tz = tbilisi();
    let r = Recurrence::Weekly {
        days: WeekDays::new([Weekday::Monday, Weekday::Friday]).unwrap(),
        at: time(18, 0, 0, 0),
    };
    // 2026-03-09 is a Monday.
    let mut cursor = at(2026, 3, 8, 0, 0, &tz);
    let mut fired = Vec::new();
    for _ in 0..3 {
        cursor = r.next_occurrence_after(cursor, &tz).unwrap();
        fired.push(local(cursor, &tz));
    }
    assert_eq!(fired[0], date(2026, 3, 9).at(18, 0, 0, 0));
    assert_eq!(fired[1], date(2026, 3, 13).at(18, 0, 0, 0));
    assert_eq!(fired[2], date(2026, 3, 16).at(18, 0, 0, 0));
}

#[test]
fn a_once_reminder_fires_then_stops() {
    let tz = tbilisi();
    let r = Recurrence::Once {
        at: date(2026, 3, 20).at(10, 0, 0, 0),
    };
    let next = r
        .next_occurrence_after(at(2026, 3, 1, 0, 0, &tz), &tz)
        .unwrap();
    assert_eq!(local(next, &tz), date(2026, 3, 20).at(10, 0, 0, 0));
    assert_eq!(
        r.next_occurrence_after(at(2026, 3, 20, 10, 1, &tz), &tz),
        None
    );
    assert!(!r.is_recurring());
}

#[test]
fn a_wall_clock_time_survives_a_spring_forward() {
    // Berlin skips 02:00-03:00 on 2026-03-29. A 10:00 reminder is unaffected, and the
    // interval across the transition is 23 hours, not 24 — the point of using wall
    // clock times rather than fixed intervals.
    let tz = berlin();
    let r = Recurrence::Daily {
        at: time(10, 0, 0, 0),
    };
    let before = r
        .next_occurrence_after(at(2026, 3, 28, 12, 0, &tz), &tz)
        .unwrap();
    assert_eq!(local(before, &tz), date(2026, 3, 29).at(10, 0, 0, 0));
    let previous = at(2026, 3, 28, 10, 0, &tz);
    let elapsed = (before - previous).total(jiff::Unit::Hour).unwrap();
    assert_eq!(elapsed, 23.0, "the day of the transition is an hour short");
}

#[test]
fn a_time_inside_a_spring_forward_gap_still_fires() {
    // 02:30 does not exist in Berlin on 2026-03-29; it must not silently vanish.
    let tz = berlin();
    let r = Recurrence::Daily {
        at: time(2, 30, 0, 0),
    };
    let next = r
        .next_occurrence_after(at(2026, 3, 29, 0, 0, &tz), &tz)
        .unwrap();
    assert_eq!(local(next, &tz), date(2026, 3, 29).at(3, 30, 0, 0));
}

#[test]
fn a_time_repeated_by_a_fall_back_fires_on_its_first_occurrence() {
    // Berlin repeats 02:00-03:00 on 2026-10-25; the earlier instant wins.
    let tz = berlin();
    let r = Recurrence::Daily {
        at: time(2, 30, 0, 0),
    };
    let next = r
        .next_occurrence_after(at(2026, 10, 25, 0, 0, &tz), &tz)
        .unwrap();
    assert_eq!(local(next, &tz), date(2026, 10, 25).at(2, 30, 0, 0));
    // The first 02:30 is still summer time, so it is 00:30 UTC.
    assert_eq!(next.to_string(), "2026-10-25T00:30:00Z");
}

#[test]
fn the_same_wall_clock_time_is_a_different_instant_in_a_different_zone() {
    let r = Recurrence::Daily {
        at: time(10, 0, 0, 0),
    };
    let now = jiff::Timestamp::from_second(1_772_000_000).unwrap();
    let tbilisi_next = r.next_occurrence_after(now, &tbilisi()).unwrap();
    let berlin_next = r.next_occurrence_after(now, &berlin()).unwrap();
    assert_ne!(tbilisi_next, berlin_next);
}

#[test]
fn an_unknown_time_zone_is_reported_rather_than_defaulted() {
    assert!(TimeZoneName::new("Asia/Tbilisi").resolve().is_ok());
    let err = TimeZoneName::new("Mars/Olympus_Mons")
        .resolve()
        .unwrap_err();
    assert!(matches!(err, StorageError::Corrupt(_)), "{err:?}");
}

fn reminder(recurrence: Recurrence) -> Reminder {
    Reminder {
        id: ReminderId::new("r1"),
        owner: UserId::new(42),
        text: "оформить invoice".into(),
        items: Vec::new(),
        recurrence,
        timezone: TimeZoneName::new("Asia/Tbilisi"),
        created_at: jiff::Timestamp::UNIX_EPOCH,
        next_fire_at: None,
        last_fired_at: None,
        acknowledged_through: None,
    }
}

#[test]
fn rescheduling_fills_in_the_next_firing_time() {
    let tz = tbilisi();
    let mut r = reminder(Recurrence::Monthly {
        days: MonthDays::new([20]).unwrap(),
        at: time(10, 0, 0, 0),
    });
    assert!(!r.is_active());
    let next = r.reschedule(at(2026, 3, 5, 9, 0, &tz)).unwrap().unwrap();
    assert_eq!(local(next, &tz), date(2026, 3, 20).at(10, 0, 0, 0));
    assert!(r.is_active());
}

#[test]
fn firing_advances_the_schedule_and_records_the_delivery() {
    let tz = tbilisi();
    let mut r = reminder(Recurrence::Monthly {
        days: MonthDays::new([20]).unwrap(),
        at: time(10, 0, 0, 0),
    });
    let fired_at = at(2026, 3, 20, 10, 0, &tz);
    let next = r.mark_fired(fired_at).unwrap().unwrap();
    assert_eq!(r.last_fired_at, Some(fired_at));
    assert_eq!(local(next, &tz), date(2026, 4, 20).at(10, 0, 0, 0));
}

#[test]
fn firing_a_one_off_leaves_it_inactive() {
    let tz = tbilisi();
    let mut r = reminder(Recurrence::Once {
        at: date(2026, 3, 20).at(10, 0, 0, 0),
    });
    assert_eq!(r.mark_fired(at(2026, 3, 20, 10, 0, &tz)).unwrap(), None);
    assert!(!r.is_active());
    assert!(r.last_fired_at.is_some());
}

#[test]
fn a_reminder_in_a_broken_zone_reports_instead_of_rescheduling() {
    let mut r = reminder(Recurrence::Daily {
        at: time(10, 0, 0, 0),
    });
    r.timezone = TimeZoneName::new("Mars/Olympus_Mons");
    assert!(r.reschedule(jiff::Timestamp::UNIX_EPOCH).is_err());
}

#[test]
fn recurrences_round_trip_through_serde() {
    for recurrence in [
        Recurrence::Once {
            at: date(2026, 3, 20).at(10, 0, 0, 0),
        },
        Recurrence::Daily {
            at: time(10, 0, 0, 0),
        },
        Recurrence::Weekly {
            days: WeekDays::new([Weekday::Friday]).unwrap(),
            at: time(9, 30, 0, 0),
        },
        Recurrence::Monthly {
            days: MonthDays::new([20]).unwrap(),
            at: time(10, 0, 0, 0),
        },
    ] {
        let json = serde_json::to_string(&recurrence).unwrap();
        assert_eq!(
            serde_json::from_str::<Recurrence>(&json).unwrap(),
            recurrence
        );
    }
}

#[test]
fn a_reminder_round_trips_through_serde() {
    let mut original = reminder(Recurrence::Monthly {
        days: MonthDays::new([20]).unwrap(),
        at: time(10, 0, 0, 0),
    });
    original
        .reschedule(at(2026, 3, 5, 9, 0, &tbilisi()))
        .unwrap();
    let json = serde_json::to_string(&original).unwrap();
    assert_eq!(serde_json::from_str::<Reminder>(&json).unwrap(), original);
}
