//! Template-driven prompt generation with **known ground truth** (Stage 1a of `finetune/PLAN.md`).
//!
//! v2 (rebalanced): the first dataset was skewed toward trivial single-events and thin on the hard
//! compositional cases, which made the fine-tune overfit and regress multi-intent/hard-task. This
//! version uses a seeded RNG to SAMPLE a controlled, balanced count per category, draws from much
//! larger slot vocabularies, and varies sentence structure (multiple phrasings per category) so the
//! student generalizes instead of memorizing one template shape. Weight is heaviest on the
//! categories the held-out battery exposed as weak: multi-intent, hard-task, correction, odd-time.

use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, Timelike, Weekday};
use pushin_lib::db;
use pushin_lib::model::Event;
use pushin_lib::parser::PlanOutcome;
use rusqlite::Connection;

pub struct SeedEvent {
    pub title: String,
    pub s_off: i64,
    pub sh: u32,
    pub sm: u32,
    pub e_off: i64,
    pub eh: u32,
    pub em: u32,
}
fn seed(title: &str, s_off: i64, sh: u32, sm: u32, e_off: i64, eh: u32, em: u32) -> SeedEvent {
    SeedEvent { title: title.into(), s_off, sh, sm, e_off, eh, em }
}

pub struct Template {
    pub category: &'static str,
    pub prompt: String,
    pub seed: Vec<SeedEvent>,
    pub history: Vec<(String, String)>,
    pub check: Box<dyn Fn(&PlanOutcome, &Connection) -> bool>,
}

// ---------------- seeded PRNG (xorshift64*) — deterministic, no external crate ----------------
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a, T>(&mut self, s: &'a [T]) -> &'a T {
        &s[self.below(s.len())]
    }
    fn chance(&mut self, p_pct: u64) -> bool {
        self.next() % 100 < p_pct
    }
}

// ---------------- check helpers ----------------
fn parse(s: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok()
}
fn events(c: &Connection) -> Vec<Event> {
    db::list_events(c).unwrap_or_default()
}
fn only_event(c: &Connection) -> Option<Event> {
    let v = events(c);
    if v.len() == 1 {
        v.into_iter().next()
    } else {
        None
    }
}
fn ev_by(c: &Connection, needle: &str) -> Option<Event> {
    let n = needle.to_lowercase();
    events(c).into_iter().find(|e| e.title.to_lowercase().contains(&n))
}
fn start_hm(e: &Event) -> Option<(u32, u32)> {
    parse(&e.start).map(|s| (s.hour(), s.minute()))
}
fn minutes(e: &Event) -> Option<i64> {
    Some((parse(&e.end)? - parse(&e.start)?).num_minutes())
}
fn date_of(e: &Event) -> Option<NaiveDate> {
    parse(&e.start).map(|s| s.date())
}
fn task_estimate_near(c: &Connection, want: i64) -> bool {
    db::list_tasks(c).map(|ts| ts.iter().any(|t| (t.estimated_minutes - want).abs() <= 15)).unwrap_or(false)
}
fn task_count(c: &Connection) -> usize {
    db::list_tasks(c).map(|t| t.len()).unwrap_or(0)
}
fn task_has_dep(c: &Connection) -> bool {
    db::list_tasks(c).map(|ts| ts.iter().any(|t| !t.depends_on.is_empty())).unwrap_or(false)
}
fn task_has_priority(c: &Connection, p: i64) -> bool {
    db::list_tasks(c).map(|ts| ts.iter().any(|t| t.priority == p)).unwrap_or(false)
}
fn weekday(s: &str) -> Weekday {
    match s {
        "monday" => Weekday::Mon,
        "tuesday" => Weekday::Tue,
        "wednesday" => Weekday::Wed,
        "thursday" => Weekday::Thu,
        "friday" => Weekday::Fri,
        "saturday" => Weekday::Sat,
        _ => Weekday::Sun,
    }
}
fn resolve_day(today: NaiveDate, day: &str) -> NaiveDate {
    match day {
        "today" => today,
        "tomorrow" => today + Duration::days(1),
        other => {
            let wd = weekday(other);
            (0..7).map(|i| today + Duration::days(i)).find(|d| d.weekday() == wd).unwrap_or(today)
        }
    }
}

// ---------------- vocabularies ----------------
const EVENT_ACTS: &[&str] = &[
    "Dentist appointment", "Doctor visit", "Team meeting", "Coffee with Alex", "Lunch with mom", "Call with the bank",
    "Haircut", "Therapy session", "1:1 with Sarah", "Standup", "Project review", "Interview", "Parent-teacher conference",
    "Dinner with the team", "Catch-up with Jordan", "Eye exam", "Car service", "Client call", "Design review", "Sync with Priya",
];
const DAYS: &[&str] = &["today", "tomorrow", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"];
// (display, hour, minute)
const TIMES: &[(&str, u32, u32)] = &[
    ("3pm", 15, 0), ("9am", 9, 0), ("10am", 10, 0), ("2pm", 14, 0), ("1pm", 13, 0), ("4pm", 16, 0), ("11am", 11, 0),
    ("8am", 8, 0), ("5pm", 17, 0), ("7pm", 19, 0), ("noon", 12, 0), ("9:30am", 9, 30), ("2:30pm", 14, 30),
    ("10:15am", 10, 15), ("4:45pm", 16, 45), ("6pm", 18, 0), ("8pm", 20, 0),
];
const DURATIONS: &[(&str, i64)] = &[
    ("for 30 minutes", 30), ("for an hour", 60), ("for 90 minutes", 90), ("for 2 hours", 120), ("for 45 minutes", 45),
    ("for 15 minutes", 15), ("for 1.5 hours", 90), ("for three hours", 180), ("for half an hour", 30),
];
const TASK_VERBS: &[(&str, i64)] = &[
    ("study for my chemistry final", 180), ("write the quarterly report", 90), ("finish the design mockups", 120),
    ("review the contract", 60), ("prepare the slides", 120), ("draft the proposal", 90), ("edit the manuscript", 120),
    ("grade the exams", 150), ("plan the offsite", 60), ("clean out the garage", 120), ("respond to emails", 30),
    ("read chapter 4", 45), ("debug the payment flow", 120), ("update the budget", 60),
];
const DEADLINES: &[&str] = &["by Friday", "by Monday", "due Wednesday", "before Thursday", "by tomorrow", "due next week", "by the end of the week"];
const HABIT_ACTS: &[&str] = &[
    "work out", "meditate", "read", "journal", "stretch", "go for a run", "practice guitar", "drink more water",
    "do yoga", "take my vitamins", "review my goals", "walk the dog", "practice Spanish",
];
const HABIT_TAILS: &[&str] = &["every day", "every morning", "every night", "daily", "each evening", "every night before bed", "first thing each morning"];

// ---------------- phrasing helpers ----------------
fn phrase_event(r: &mut Rng, act: &str, day: &str, time: &str) -> String {
    match r.below(6) {
        0 => format!("{act} {day} at {time}."),
        1 => format!("I have {act} {day} at {time}."),
        2 => format!("Schedule {act} for {day} at {time}."),
        3 => format!("Put {act} on my calendar {day} at {time}."),
        4 => format!("{day} I've got {act} at {time}."),
        _ => format!("Add {act} {day} at {time}."),
    }
}
fn phrase_range(r: &mut Rng, act: &str, day: &str, t1: &str, t2: &str) -> String {
    match r.below(4) {
        0 => format!("{act} {day} from {t1} to {t2}."),
        1 => format!("{act} {day} {t1} to {t2}."),
        2 => format!("Block {t1}-{t2} {day} for {act}."),
        _ => format!("{act} {day}, {t1} until {t2}."),
    }
}
fn phrase_task(r: &mut Rng, task: &str, dur: &str, deadline: Option<&str>) -> String {
    let d = deadline.map(|x| format!(" {x}")).unwrap_or_default();
    match r.below(4) {
        0 => format!("I need to {task}, about {dur}{d}."),
        1 => format!("I have to {task} ({dur}){d}."),
        2 => format!("Remind me to {task}, roughly {dur}{d}."),
        _ => format!("{task}{d} — should take about {dur}.").replace("study", "Study").replace("write", "Write"),
    }
}
fn dur_words(mins: i64) -> &'static str {
    match mins {
        10 => "10 minutes",
        15 => "15 minutes",
        20 => "20 minutes",
        30 => "30 minutes",
        45 => "45 minutes",
        60 => "an hour",
        90 => "90 minutes",
        120 => "2 hours",
        150 => "2.5 hours",
        180 => "3 hours",
        _ => "an hour",
    }
}

// ---------------- generators ----------------
pub fn all() -> Vec<Template> {
    let today = Local::now().naive_local().date();
    let mut r = Rng::new(0xC0FFEE_1234_5678);
    let mut out: Vec<Template> = Vec::new();

    // 1) Single event at a time — varied phrasing, days, time formats.
    for _ in 0..120 {
        let act = (*r.pick(EVENT_ACTS)).to_string();
        let day = *r.pick(DAYS);
        let (ts, h, m) = *r.pick(TIMES);
        let date = resolve_day(today, day);
        let prompt = phrase_event(&mut r, &act, day, ts);
        out.push(Template {
            category: "single-event",
            prompt,
            seed: vec![],
            history: vec![],
            check: Box::new(move |o, c| {
                o.created_event_titles.len() == 1
                    && only_event(c).map(|e| start_hm(&e) == Some((h, m)) && date_of(&e) == Some(date)).unwrap_or(false)
            }),
        });
    }

    // 2) Event with explicit range.
    for _ in 0..110 {
        let act = (*r.pick(EVENT_ACTS)).to_string();
        let day = *r.pick(DAYS);
        // build a range with a known duration
        let starts: &[(&str, u32, u32, &str, u32, u32, i64)] = &[
            ("9am", 9, 0, "10:30am", 10, 30, 90),
            ("1pm", 13, 0, "3pm", 15, 0, 120),
            ("10am", 10, 0, "11am", 11, 0, 60),
            ("2pm", 14, 0, "2:30pm", 14, 30, 30),
            ("11am", 11, 0, "12:30pm", 12, 30, 90),
            ("3pm", 15, 0, "4pm", 16, 0, 60),
        ];
        let (t1, h, m, t2, _eh, _em, dur) = *r.pick(starts);
        let prompt = phrase_range(&mut r, &act, day, t1, t2);
        out.push(Template {
            category: "event-range",
            prompt,
            seed: vec![],
            history: vec![],
            check: Box::new(move |o, c| {
                o.created_event_titles.len() == 1
                    && only_event(c).map(|e| start_hm(&e) == Some((h, m)) && minutes(&e) == Some(dur)).unwrap_or(false)
            }),
        });
    }

    // 3) Event with a stated duration.
    for _ in 0..110 {
        let act = (*r.pick(EVENT_ACTS)).to_string();
        let day = *r.pick(DAYS);
        let (ts, h, m) = *r.pick(TIMES);
        let (durs, dur) = *r.pick(DURATIONS);
        let prompt = format!("{} {durs}.", phrase_event(&mut r, &act, day, ts).trim_end_matches('.'));
        out.push(Template {
            category: "event-duration",
            prompt,
            seed: vec![],
            history: vec![],
            check: Box::new(move |o, c| {
                o.created_event_titles.len() == 1
                    && only_event(c).map(|e| start_hm(&e) == Some((h, m)) && minutes(&e) == Some(dur)).unwrap_or(false)
            }),
        });
    }

    // 4) Multi-day event with a daily window → N separate same-time days.
    for _ in 0..50 {
        let comp = *r.pick(&["robotics competition", "conference", "training retreat", "music festival", "trade show", "workshop series", "hackathon", "sales summit"]);
        let (disp, key) = *r.pick(&[("this Thursday", "thursday"), ("Monday", "monday"), ("tomorrow", "tomorrow"), ("next Wednesday", "wednesday"), ("Friday", "friday")]);
        let (n, range, h, mins) = *r.pick(&[(3i64, "8 am to 5 pm", 8u32, 540i64), (2, "9am to 4pm", 9, 420), (4, "10am to 3pm", 10, 300), (3, "9 to 5", 9, 480)]);
        let first = resolve_day(today, key);
        let prompt = match r.below(3) {
            0 => format!("From {disp}, I have a {comp} that lasts for {n} days from {range}."),
            1 => format!("I've got a {comp} starting {disp} for {n} days, {range}."),
            _ => format!("{comp} {disp} — {n} days, {range} each day."),
        };
        out.push(Template {
            category: "multiday-window",
            prompt,
            seed: vec![],
            history: vec![],
            check: Box::new(move |_o, c| {
                let evs = events(c);
                if evs.len() != n as usize {
                    return false;
                }
                if !evs.iter().all(|e| start_hm(e) == Some((h, 0)) && minutes(e) == Some(mins)) {
                    return false;
                }
                let mut ds: Vec<NaiveDate> = evs.iter().filter_map(date_of).collect();
                ds.sort();
                ds.dedup();
                ds.len() == n as usize && ds[0] == first && (ds[ds.len() - 1] - ds[0]).num_days() == n - 1
            }),
        });
    }

    // 5) Single task with an estimate (a task, NOT a fixed event).
    for _ in 0..70 {
        let (task, est) = *r.pick(TASK_VERBS);
        let deadline = if r.chance(50) { Some(*r.pick(DEADLINES)) } else { None };
        let prompt = phrase_task(&mut r, task, dur_words(est), deadline);
        out.push(Template {
            category: "single-task",
            prompt,
            seed: vec![],
            history: vec![],
            check: Box::new(move |o, c| o.created_event_titles.is_empty() && task_estimate_near(c, est)),
        });
    }

    // 6) Multi-step project decomposition.
    let projects: &[&str] = &[
        "Launch a side project in 3 weeks: design a logo, build a landing page, write 3 blog posts, and set up analytics.",
        "Prep for my exam Friday: review 4 chapters, do 2 practice tests, and make a cheat sheet.",
        "Plan the launch: write the press release, line up 3 demos, and schedule the announcement.",
        "Organize the move: book a truck, pack the kitchen, change my address, and cancel utilities.",
        "Get ready for the conference: finalize the deck, print handouts, and rehearse the talk.",
        "Set up the new laptop: install the tools, restore my files, and configure the editor.",
    ];
    for p in projects {
        out.push(Template {
            category: "multi-task",
            prompt: (*p).into(),
            seed: vec![],
            history: vec![],
            check: Box::new(|_o, c| task_count(c) >= 3 && events(c).is_empty()),
        });
    }

    // 7) Habits — one daily habit, never a one-off event.
    for _ in 0..80 {
        let act = *r.pick(HABIT_ACTS);
        let tail = *r.pick(HABIT_TAILS);
        let dur = if r.chance(50) { format!(" for {}", dur_words(*r.pick(&[10i64, 15, 20, 30, 45]))) } else { String::new() };
        let prompt = match r.below(3) {
            0 => format!("I'm going to {act} {tail}{dur}."),
            1 => format!("Remind me to {act} {tail}{dur}."),
            _ => format!("I want to {act} {tail}{dur}."),
        };
        out.push(Template {
            category: "habit",
            prompt,
            seed: vec![],
            history: vec![],
            check: Box::new(|o, _c| !o.created_habit_names.is_empty() && o.created_event_titles.is_empty()),
        });
    }

    // 8) Edits: move / shorten / rename / shift — seeded.
    for _ in 0..110 {
        let title = *r.pick(&["Dentist", "Workshop", "Meeting", "Standup", "Gym session", "Review"]);
        let t = title.to_string();
        match r.below(4) {
            0 => {
                // move to a new time
                let (nt, nh, nm) = *r.pick(TIMES);
                let prompt = match r.below(3) {
                    0 => format!("Move the {} to {nt}.", title.to_lowercase()),
                    1 => format!("Reschedule my {} to {nt}.", title.to_lowercase()),
                    _ => format!("Can you push the {} to {nt}?", title.to_lowercase()),
                };
                out.push(Template {
                    category: "edit",
                    prompt,
                    seed: vec![seed(title, 1, 14, 0, 1, 15, 0)],
                    history: vec![],
                    check: Box::new(move |o, c| !o.updated_event_titles.is_empty() && ev_by(c, &t).and_then(|e| start_hm(&e)) == Some((nh, nm))),
                });
            }
            1 => {
                // change length
                let (durs, dur) = *r.pick(&[("2 hours", 120i64), ("90 minutes", 90), ("30 minutes", 30), ("an hour", 60)]);
                out.push(Template {
                    category: "edit",
                    prompt: format!("Make the {} {durs}.", title.to_lowercase()),
                    seed: vec![seed(title, 1, 10, 0, 1, 11, 0)],
                    history: vec![],
                    check: Box::new(move |o, c| !o.updated_event_titles.is_empty() && ev_by(c, &t).and_then(|e| minutes(&e)) == Some(dur)),
                });
            }
            2 => {
                // rename
                let newname = *r.pick(&["Morning workout", "Planning session", "Quick sync", "Deep work"]);
                let nn = newname.to_lowercase();
                let words: Vec<String> = nn.split_whitespace().map(|s| s.to_string()).collect();
                out.push(Template {
                    category: "edit",
                    prompt: format!("Rename my {} to {newname}.", title.to_lowercase()),
                    seed: vec![seed(title, 1, 9, 0, 1, 10, 0)],
                    history: vec![],
                    check: Box::new(move |o, c| {
                        !o.updated_event_titles.is_empty() && events(c).iter().any(|e| words.iter().all(|w| e.title.to_lowercase().contains(w)))
                    }),
                });
            }
            _ => {
                // relative shift
                let (phr, delta) = *r.pick(&[("back an hour", 60i64), ("up 30 minutes", -30), ("back 30 minutes", 30), ("forward an hour", 60)]);
                let base = (14i64, 0i64);
                let nm = (base.1 + delta).rem_euclid(60) as u32;
                let nh = (base.0 + (base.1 + delta).div_euclid(60)) as u32;
                let prompt = format!("Push my {} {phr}.", title.to_lowercase());
                out.push(Template {
                    category: "edit",
                    prompt,
                    seed: vec![seed(title, 1, 14, 0, 1, 15, 0)],
                    history: vec![],
                    check: Box::new(move |o, c| !o.updated_event_titles.is_empty() && ev_by(c, &t).and_then(|e| start_hm(&e)) == Some((nh, nm))),
                });
            }
        }
    }

    // 9) Removes — single, selective, plural.
    for _ in 0..60 {
        match r.below(3) {
            0 => {
                let title = *r.pick(&["Sleepover", "Standup", "Dentist", "Haircut", "Meeting"]);
                let prompt = match r.below(3) {
                    0 => format!("Cancel the {}.", title.to_lowercase()),
                    1 => format!("Remove my {}.", title.to_lowercase()),
                    _ => format!("Delete the {} from my calendar.", title.to_lowercase()),
                };
                out.push(Template {
                    category: "remove",
                    prompt,
                    seed: vec![seed(title, 1, 20, 0, 1, 21, 0)],
                    history: vec![],
                    check: Box::new(|o, _c| !o.removed_event_titles.is_empty() && o.created_event_titles.is_empty()),
                });
            }
            1 => {
                // selective: spare the sibling
                out.push(Template {
                    category: "remove",
                    prompt: "Cancel lunch with Dan.".into(),
                    seed: vec![seed("Lunch with mom", 1, 12, 0, 1, 13, 0), seed("Lunch with Dan", 1, 15, 0, 1, 16, 0)],
                    history: vec![],
                    check: Box::new(|o, c| o.removed_event_titles.iter().any(|t| t.to_lowercase().contains("dan")) && ev_by(c, "with mom").is_some()),
                });
            }
            _ => {
                // plural: all of a kind
                out.push(Template {
                    category: "remove",
                    prompt: "Cancel all my sleepovers.".into(),
                    seed: vec![seed("Sleepover at Jake's", 2, 20, 0, 3, 8, 0), seed("Sleepover at Mia's", 4, 20, 0, 5, 8, 0)],
                    history: vec![],
                    check: Box::new(|o, _c| o.removed_event_titles.len() >= 2),
                });
            }
        }
    }

    // 10) Restraint — greetings/queries/past/venting → create NOTHING.
    let restraint: &[&str] = &[
        "Hey! How's it going today?", "What's on my calendar tomorrow?", "I already finished the laundry earlier.",
        "Honestly I'm feeling pretty overwhelmed this week.", "I have some stuff going on next week.", "Thanks so much for the help!",
        "Can you remind me what a habit is?", "ugh mondays", "good morning", "what can you do?", "I went for a run yesterday.",
        "do you think I'm too busy?", "lol that meeting was rough", "no plans for me", "just checking in",
    ];
    for _ in 0..50 {
        let p = *r.pick(restraint);
        out.push(Template {
            category: "restraint",
            prompt: p.into(),
            seed: vec![],
            history: vec![],
            check: Box::new(|o, _c| {
                o.created_event_titles.is_empty() && o.created_task_ids.is_empty() && o.created_habit_names.is_empty()
                    && o.updated_event_titles.is_empty() && o.removed_event_titles.is_empty()
            }),
        });
    }

    // 11) Odd / colloquial time formats.
    for _ in 0..80 {
        let act = (*r.pick(EVENT_ACTS)).to_string();
        let day = *r.pick(DAYS);
        let (phr, h, m) = *r.pick(&[
            ("at 1400", 14u32, 0u32), ("at 0930", 9, 30), ("at noon", 12, 0), ("at midnight", 0, 0), ("at 12pm", 12, 0),
            ("at 12am", 0, 0), ("at half past 2 in the afternoon", 14, 30), ("at a quarter past 9 in the morning", 9, 15),
            ("at 6 in the morning", 6, 0), ("at 8 in the evening", 20, 0), ("at 0800", 8, 0), ("at 1730", 17, 30),
            ("at 7 sharp tonight", 19, 0), ("first thing, 8am", 8, 0),
        ]);
        let prompt = format!("{act} {day} {phr}.");
        out.push(Template {
            category: "odd-time",
            prompt,
            seed: vec![],
            history: vec![],
            check: Box::new(move |o, c| {
                o.created_event_titles.len() == 1 && only_event(c).and_then(|e| start_hm(&e)) == Some((h, m))
            }),
        });
    }

    // 12) Relative dates (Rust recovery owns these → exact).
    for _ in 0..40 {
        let act = (*r.pick(EVENT_ACTS)).to_string();
        let (spec, off) = *r.pick(&[("the day after tomorrow", 2i64), ("in two weeks", 14), ("in 3 days", 3), ("in a week", 7), ("in 10 days", 10), ("in two days", 2)]);
        let (ts, h, m) = *r.pick(TIMES);
        let want = today + Duration::days(off);
        let prompt = format!("{act} {spec} at {ts}.");
        out.push(Template {
            category: "rel-date",
            prompt,
            seed: vec![],
            history: vec![],
            check: Box::new(move |o, c| {
                o.created_event_titles.len() == 1
                    && only_event(c).map(|e| date_of(&e) == Some(want) && start_hm(&e) == Some((h, m))).unwrap_or(false)
            }),
        });
    }

    // 13) Day-of-month ordinals.
    for _ in 0..30 {
        let act = (*r.pick(EVENT_ACTS)).to_string();
        let dom = *r.pick(&[3u32, 5, 12, 15, 20, 25, 28]);
        let (ts, h, m) = *r.pick(TIMES);
        let suffix = match dom % 10 {
            1 if dom != 11 => "st",
            2 if dom != 12 => "nd",
            3 if dom != 13 => "rd",
            _ => "th",
        };
        let prompt = format!("{act} on the {dom}{suffix} at {ts}.");
        out.push(Template {
            category: "day-of-month",
            prompt,
            seed: vec![],
            history: vec![],
            check: Box::new(move |o, c| {
                o.created_event_titles.len() == 1
                    && only_event(c).map(|e| date_of(&e).map(|d| d.day()) == Some(dom) && start_hm(&e) == Some((h, m))).unwrap_or(false)
            }),
        });
    }

    // 14) Several events in one line (positional ranges/times).
    for _ in 0..60 {
        let starts: &[(&str, &str, i64, &str, &str, i64)] = &[
            ("standup 9-9:15", "standup", 15, "review 11-12", "review", 60),
            ("lunch 12-1", "lunch", 60, "a call 3-4", "call", 60),
            ("coffee 9-9:30", "coffee", 30, "a sync 2-3", "sync", 60),
            ("gym 7-8am", "gym", 60, "dinner 7-9pm", "dinner", 120),
        ];
        let (p1, n1, m1, p2, n2, m2) = *r.pick(starts);
        let day = *r.pick(&["today", "tomorrow", "friday", "monday"]);
        let (n1, m1, n2, m2) = (n1.to_string(), m1, n2.to_string(), m2);
        let prompt = match r.below(2) {
            0 => format!("{day}: {p1} and {p2}."),
            _ => format!("On {day} I have {p1}, then {p2}."),
        };
        out.push(Template {
            category: "multi-event",
            prompt,
            seed: vec![],
            history: vec![],
            check: Box::new(move |o, c| {
                o.created_event_titles.len() == 2
                    && ev_by(c, &n1).and_then(|e| minutes(&e)) == Some(m1)
                    && ev_by(c, &n2).and_then(|e| minutes(&e)) == Some(m2)
            }),
        });
    }

    // 15) Mid-sentence self-correction → ONE event at the FINAL time.
    for _ in 0..60 {
        let act = (*r.pick(EVENT_ACTS)).to_string();
        let day = *r.pick(DAYS);
        let (_t1, _, _) = *r.pick(TIMES);
        let t1 = *r.pick(&["2pm", "10am", "3pm", "1pm", "9am"]);
        let (t2, h2, m2) = *r.pick(&[("4pm", 16u32, 0u32), ("3pm", 15, 0), ("11am", 11, 0), ("5pm", 17, 0), ("noon", 12, 0)]);
        if t1 == t2 {
            continue;
        }
        let prompt = match r.below(3) {
            0 => format!("Schedule {act} {day} at {t1}, actually make it {t2}."),
            1 => format!("{act} {day} at {t1} — no wait, {t2}."),
            _ => format!("Put {act} {day} at {t1}. Hmm, actually {t2} works better."),
        };
        out.push(Template {
            category: "correction",
            prompt,
            seed: vec![],
            history: vec![],
            check: Box::new(move |o, c| o.created_event_titles.len() == 1 && only_event(c).and_then(|e| start_hm(&e)) == Some((h2, m2))),
        });
    }

    // 16) Mixed: event + task in one message.
    for _ in 0..70 {
        let act = (*r.pick(EVENT_ACTS)).to_string();
        let day = *r.pick(DAYS);
        let (ts, h, m) = *r.pick(TIMES);
        let (task, est) = *r.pick(TASK_VERBS);
        let date = resolve_day(today, day);
        let prompt = format!("{act} {day} at {ts}, and I need to {task}, about {}.", dur_words(est));
        out.push(Template {
            category: "mixed",
            prompt,
            seed: vec![],
            history: vec![],
            check: Box::new(move |o, c| {
                !o.created_event_titles.is_empty()
                    && events(c).iter().any(|e| start_hm(e) == Some((h, m)) && date_of(e) == Some(date))
                    && task_estimate_near(c, est)
            }),
        });
    }

    // 17) MULTI-INTENT (the weak spot) — 2–3 intents per message, many combos. Heaviest category.
    for _ in 0..170 {
        let act = (*r.pick(EVENT_ACTS)).to_string();
        let day = *r.pick(DAYS);
        let (ts, h, m) = *r.pick(TIMES);
        let date = resolve_day(today, day);
        let (task, est) = *r.pick(TASK_VERBS);
        let hact = (*r.pick(HABIT_ACTS)).to_string();
        let htail = *r.pick(HABIT_TAILS);
        let ev_ok = move |c: &Connection| events(c).iter().any(|e| start_hm(e) == Some((h, m)) && date_of(e) == Some(date));
        match r.below(5) {
            0 => {
                // event + habit
                let prompt = format!("Book {act} {day} at {ts}, and remind me to {hact} {htail}.");
                out.push(Template {
                    category: "multi-intent",
                    prompt,
                    seed: vec![],
                    history: vec![],
                    check: Box::new(move |o, c| !o.created_habit_names.is_empty() && ev_ok(c)),
                });
            }
            1 => {
                // event + task
                let prompt = format!("{act} {day} at {ts}, and I need to {task} ({}).", dur_words(est));
                out.push(Template {
                    category: "multi-intent",
                    prompt,
                    seed: vec![],
                    history: vec![],
                    check: Box::new(move |o, c| ev_ok(c) && task_estimate_near(c, est)),
                });
            }
            2 => {
                // task + habit
                let prompt = format!("I need to {task}, about {}, and {hact} {htail}.", dur_words(est));
                out.push(Template {
                    category: "multi-intent",
                    prompt,
                    seed: vec![],
                    history: vec![],
                    check: Box::new(move |o, c| !o.created_habit_names.is_empty() && task_estimate_near(c, est) && o.created_event_titles.is_empty()),
                });
            }
            3 => {
                // event + task + habit (triple)
                let prompt = format!("{act} {day} at {ts}; I need to {task}, about {}; and {hact} {htail}.", dur_words(est));
                out.push(Template {
                    category: "multi-intent",
                    prompt,
                    seed: vec![],
                    history: vec![],
                    check: Box::new(move |o, c| !o.created_habit_names.is_empty() && task_estimate_near(c, est) && ev_ok(c)),
                });
            }
            _ => {
                // remove + create (seeded)
                let old = *r.pick(&["Old sync", "Standup", "Weekly review"]);
                let prompt = format!("Cancel the {}, and schedule {act} {day} at {ts}.", old.to_lowercase());
                out.push(Template {
                    category: "multi-intent",
                    prompt,
                    seed: vec![seed(old, 1, 10, 0, 1, 10, 30)],
                    history: vec![],
                    check: Box::new(move |o, c| !o.removed_event_titles.is_empty() && ev_ok(c)),
                });
            }
        }
    }

    // 18) HARD-TASK — dependency chains, mixed priorities, distinct estimates. Heavily weighted.
    for _ in 0..100 {
        match r.below(3) {
            0 => {
                // sequential dependencies
                let chains: &[&str] = &[
                    "To launch the app I need to fix the login bug, then write tests, then deploy to prod.",
                    "Before the release: merge the branch, then run QA, then tag the version.",
                    "For dinner: buy groceries, then prep the veggies, then cook the main.",
                    "To publish: write the draft, then get it reviewed, then format and post it.",
                ];
                out.push(Template {
                    category: "hard-task",
                    prompt: (*r.pick(chains)).into(),
                    seed: vec![],
                    history: vec![],
                    check: Box::new(|_o, c| task_count(c) >= 3 && task_has_dep(c)),
                });
            }
            1 => {
                // mixed priorities
                let prompts: &[&str] = &[
                    "Urgent: file the tax forms by tomorrow. Also, low priority — organize the old photos sometime.",
                    "ASAP I have to submit the grant. Whenever I get to it, I should tidy my inbox.",
                    "Critical: patch the security hole today. Low priority: rename the old files.",
                ];
                out.push(Template {
                    category: "hard-task",
                    prompt: (*r.pick(prompts)).into(),
                    seed: vec![],
                    history: vec![],
                    check: Box::new(|_o, c| task_has_priority(c, 4) && task_has_priority(c, 1)),
                });
            }
            _ => {
                // two distinct estimates
                let (a, ea) = *r.pick(TASK_VERBS);
                let (b, eb) = *r.pick(&[("email it to the team", 10i64), ("send the invites", 15), ("post the summary", 20)]);
                if ea == eb {
                    continue;
                }
                out.push(Template {
                    category: "hard-task",
                    prompt: format!("{a}, about {}, and {b}, {}.", dur_words(ea), dur_words(eb)).replace("study", "Study").replace("write", "Write").replace("finish", "Finish").replace("review", "Review").replace("prepare", "Prepare").replace("draft", "Draft").replace("edit", "Edit").replace("grade", "Grade").replace("plan", "Plan").replace("clean", "Clean").replace("respond", "Respond").replace("read", "Read").replace("debug", "Debug").replace("update", "Update"),
                    seed: vec![],
                    history: vec![],
                    check: Box::new(move |_o, c| task_estimate_near(c, ea) && task_estimate_near(c, eb)),
                });
            }
        }
    }

    // 19) Title with ampersand (HTML-escape recovery).
    for _ in 0..20 {
        let (act, needle) = *r.pick(&[("Lunch at AT&T", "at&t"), ("Meeting about R&D", "r&d"), ("Call with Procter & Gamble", "&"), ("Dinner at Q&A bistro", "q&a")]);
        let day = *r.pick(DAYS);
        let (ts, _, _) = *r.pick(TIMES);
        out.push(Template {
            category: "title-amp",
            prompt: format!("{act} {day} at {ts}."),
            seed: vec![],
            history: vec![],
            check: Box::new(move |o, c| {
                o.created_event_titles.len() == 1 && only_event(c).map(|e| e.title.to_lowercase().contains(needle)).unwrap_or(false)
            }),
        });
    }

    // 20) Context follow-ups (history-dependent).
    for _ in 0..50 {
        let (ts, h, m) = *r.pick(TIMES);
        let day = *r.pick(&["friday", "tuesday", "monday", "thursday"]);
        match r.below(2) {
            0 => {
                // create via history (prior turn established the subject)
                let subj = *r.pick(&["a meeting with Sarah", "lunch with the team", "a call with the client", "a 1:1 with my manager"]);
                out.push(Template {
                    category: "context",
                    prompt: format!("This {day} at {ts}."),
                    seed: vec![],
                    history: vec![("user".into(), format!("Can you schedule {subj}?")), ("assistant".into(), "Sure — what day and time?".into())],
                    check: Box::new(move |o, c| o.created_event_titles.len() == 1 && only_event(c).and_then(|e| start_hm(&e)) == Some((h, m))),
                });
            }
            _ => {
                // edit via history
                out.push(Template {
                    category: "context",
                    prompt: format!("Actually, make it {ts} instead."),
                    seed: vec![seed("Dentist", 2, 14, 0, 2, 15, 0)],
                    history: vec![("user".into(), "Add a dentist appointment Friday at 2pm.".into()), ("assistant".into(), "Added Dentist on Friday at 2pm.".into())],
                    check: Box::new(move |o, c| !o.updated_event_titles.is_empty() && ev_by(c, "dentist").and_then(|e| start_hm(&e)) == Some((h, m))),
                });
            }
        }
    }

    out
}
