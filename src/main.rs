use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use clap::{Parser, Subcommand};
use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Confirm};
use ics::properties::{Description, DtEnd, DtStart, Location, Status, Summary};
use ics::{Event, ICalendar};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use serde_json;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Error, ErrorKind, Seek, SeekFrom};
use std::path::PathBuf;
use std::process::Command;
use std::env::var;
use uuid::Uuid;
use toml::{from_str, to_string_pretty};
use xdg::BaseDirectories;
use iana_time_zone;

const APP_NAME: &str = "plantrack";
const DEFAULT_CONFIG_FILE: &str = "config.toml";

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Commands,

    /// Path to the config file.
    #[arg(short, long)]
    config_file: Option<PathBuf>,

    /// Rounding interval in minutes.
    #[arg(short, long)]
    rounding: Option<u32>,

    /// Timezone for displaying events (e.g., "America/New_York").
    #[arg(long, short)]
    timezone: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Add a new event to the schedule.
    Add {
        /// Project and task, separated by a colon. Example: "ProjectA:TaskB"
        project_task: String,

        /// Timespan in the format HH:MM-HH:MM.
        /// Example: "14:30-15:00"
        timespan: String,

        /// Date in the format YYYY-MM-DD.
        /// Example: "2024-03-16"
        #[arg(long, short)]
        date: Option<String>,

        /// Optional note for the event.
        #[arg(long, short)]
        note: Option<String>,

        /// Optional location for the event.
        #[arg(short, long)]
        location: Option<String>,

        /// Mark event as booked.
        #[arg(short, long)]
        booked: bool,
    },
    /// Quickly add a new booked event for the current time.
    Quickadd {
        /// Project and task, separated by a colon. Example: "ProjectA:TaskB"
        project_task: String,

        /// Duration of the event in minutes. Defaults to the rounding interval.
        #[arg(short, long)]
        minutes: Option<u32>,

        /// Optional note for the event.
        #[arg(short, long)]
        note: Option<String>,

        /// Optional location for the event.
        #[arg(short, long)]
        location: Option<String>,

        /// Optional to apply next minutes, default is past minutes.
        #[arg(short, long)]
        forward: bool,
    },
    /// Add a todo item to the schedule.
    Todo {
        /// Project and task, separated by a colon. Example: "ProjectA:TaskB"
        project_task: String,

        /// Duration of the todo item in minutes. Defaults to 2 * rounding interval.
        #[arg(short, long)]
        minutes: Option<u32>,

        /// Book a todo in project:task.
        #[arg(short = 'i', long, value_name = "PROJECT:TASK")]
        in_project_task: Option<String>,

        /// Date in the format YYYY-MM-DD.
        /// Example: "2024-03-16"
        #[arg(long, short)]
        date: Option<String>,

        /// Timeslot in the format HH:MM-HH:MM (optional, defaults to 08:00-17:00).
        #[arg(short, long, default_value = "08:00-17:00")]
        timespan: String,

        /// Optional note for the event.
        #[arg(long, short)]
        note: Option<String>,

        /// Optional location for the event.
        #[arg(short, long)]
        location: Option<String>,
    },
    /// List all scheduled events.
    List {
        /// Number of days to look back (default: 1).
        #[arg(short, long, default_value_t = 1)]
        past_days: u32,
        /// Number of days to look forward (default: 4).
        #[arg(short, long, default_value_t = 4)]
        future_days: u32,
        /// Date for the listing in YYYY-MM-DD format. Defaults to today.
        #[arg(long)]
        date: Option<String>,
        /// Show a summary of projects and their total time.
        #[arg(short, long)]
        summary: bool,
    },
    /// Generate a report for a specific project.
    Report {
        /// The project to generate the report for.
        project: String,

        /// Reporting month. Defaults to current month
        #[arg(short, long)]
        month: Option<u32>,

        /// Reporting year. Defaults to current year
        #[arg(short, long)]
        year: Option<i32>,

        /// Target time in hours for the period (e.g., 10.5 for 10 hours and 30 minutes).
        #[arg(short, long, value_name = "HOURS")]
        target: Option<f64>,
    },
    /// Check if a time slot is free.
    Free {
        /// Timespan in the format HH:MM-HH:MM.
        /// Example: "14:30-15:00"
        timespan: String,

        /// Date in the format YYYY-MM-DD.
        /// Example: "2024-03-16"
        #[arg(long, short)]
        date: Option<String>,
    },
    /// Show the current project:task.
    Current {},
    /// Push by running a push_command if present in the config file
    Push {
    },
    /// Remove events older than a specified number of days.
    Cleanup {
        /// Number of days old events to be removed.
        days: u32,
    },
    /// Modify an existing event.
    Set {
        /// The ID of the event to modify.
        id: String,

        /// Optional Timespan in the format HH:MM-HH:MM.
        /// Example: "14:30-15:00"
        #[arg(short, long)]
        timespan: Option<String>,

        /// Optional Date in the format YYYY-MM-DD.
        /// Example: "2024-03-16"
        #[arg(long, short)]
        date: Option<String>,

        /// Optional location for the event.
        #[arg(short, long)]
        location: Option<String>,

        /// Optional note for the event.
        #[arg(long, short)]
        note: Option<String>,

        /// Mark event as booked.
        #[arg(short, long)]
        booked: Option<bool>,
    },
    /// Delete an event by ID.
    Delete {
        /// The ID of the event to delete.
        id: String,

        /// Timespan in the format HH:MM-HH:MM to remove from the event.
        #[arg(short, long)]
        timespan: Option<String>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ScheduleEvent {
    id: String,
    #[serde(with = "chrono::serde::ts_seconds")]
    start_time: DateTime<Utc>,
    #[serde(with = "chrono::serde::ts_seconds")]
    end_time: DateTime<Utc>,
    summary: String,
    note: Option<String>,
    location: Option<String>,
    booked: bool,
}

impl PartialEq for ScheduleEvent {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl ScheduleEvent {
    fn can_merge(&self, other: &Self) -> bool {
        self.summary == other.summary
            && self.note == other.note
            && self.location == other.location
            && self.booked == other.booked
            && self.end_time == other.start_time
    }
}

#[derive(Deserialize, Serialize, Debug)]
struct Config {
    schedule_file: PathBuf,
    ics_file: PathBuf,
    timezone: Option<String>,
    export_notes: Option<bool>,
    rounding: Option<u32>,
    push_command: Option<String>,
}

impl Config {
    fn load(config_file: &PathBuf) -> Result<Self, Error> {
        let xdg_dirs = BaseDirectories::with_prefix(APP_NAME)?;

        if !config_file.exists() {
            // Create default config files if they don't exist
            let config_parent = config_file.parent().unwrap_or(config_file);
            let (default_schedule_file, default_ics_file) =
                if config_parent == xdg_dirs.get_config_home() {
                    (xdg_dirs.place_data_file("schedule.json")?, xdg_dirs.place_data_file("schedule.ics")?)
                } else {
                    (config_parent.join("schedule.json"), config_parent.join("schedule.ics"))
                };

            // Ensure data directory exists
            std::fs::create_dir_all(default_schedule_file.parent().unwrap_or(&default_schedule_file))?;
            std::fs::create_dir_all(default_ics_file.parent().unwrap_or(&default_ics_file))?;


            let default_config = Self {
                schedule_file: default_schedule_file,
                ics_file: default_ics_file,
                export_notes: Some(true),
                rounding: Some(15),
                timezone: None,
                push_command: None,
            };

            std::fs::create_dir_all(config_parent)?; // Ensure config directory exists
            let config_str = to_string_pretty(&default_config).map_err(|e| {
                Error::new(ErrorKind::Other, format!("Failed to serialize default config: {}", e))
            })?;
            std::fs::write(&config_file, config_str)?;


            println!("Created default config file at: {}", config_file.display());
            return Ok(default_config);
        }

        // Load config if it exists
        let config_str = std::fs::read_to_string(&config_file)?;
        let config = from_str(&config_str).map_err(|e| {
            Error::new(ErrorKind::InvalidData, format!("Invalid config file: {}", e))
        })?;

        Ok(config)
    }
}

fn round_time_to_interval(time: NaiveTime, interval: u32, round_up: bool) -> NaiveTime {
    let minute = time.minute();
    let remainder = minute % interval;

    let new_minute = if remainder == 0 {
        minute
    } else if round_up {
        minute + (interval - remainder)
    } else {
        minute - remainder
    };

    let mut new_time = time;

    if new_minute >= 60 {
        let hour_offset = new_minute / 60;
        let new_hour = (time.hour() + hour_offset) % 24;
        let new_minute = new_minute % 60;
        new_time = new_time
            .with_hour(new_hour)
            .unwrap()
            .with_minute(new_minute)
            .unwrap();
    } else {
        new_time = new_time.with_minute(new_minute).unwrap();
    }

    new_time.with_second(0).unwrap()
}

fn parse_datetime(time_str: &str, date: Option<NaiveDate>, timezone: &Tz) -> Result<DateTime<Tz>, Error> {
    let date = date.unwrap_or_else(|| Utc::now().with_second(0).unwrap().with_nanosecond(0).unwrap().with_timezone(timezone).date_naive());
    NaiveTime::parse_from_str(time_str, "%H:%M")
        .map(|time| date.and_time(time))
        .map(|naive_datetime| timezone.from_local_datetime(&naive_datetime).unwrap())
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "Invalid time format"))
}

fn parse_datetime_range(timespan: &str, date_str: Option<&str>, interval: u32, timezone: &Tz) -> Result<(DateTime<Utc>, DateTime<Utc>), Error> {
    let (start_str, end_str) = timespan.rsplit_once('-').ok_or(Error::new(ErrorKind::InvalidInput, "Invalid timespan format"))?;

    let date = if let Some(date_str) = date_str {
        Some(NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "Invalid date format"))?)
    } else {
        None
    };

    let start_datetime_local = parse_datetime(start_str, date, timezone)?;
    let mut end_datetime_local = parse_datetime(end_str, date, timezone)?;

    // Handle overnight events
    if end_datetime_local < start_datetime_local {
        end_datetime_local = end_datetime_local + Duration::days(1);
    }


    let start_time_rounded = round_time_to_interval(start_datetime_local.time(), interval, false);
    let end_time_rounded = round_time_to_interval(end_datetime_local.time(), interval, true);

    let start_time = start_datetime_local.with_time(start_time_rounded).unwrap().with_timezone(&Utc);
    let end_time = end_datetime_local.with_time(end_time_rounded).unwrap().with_timezone(&Utc);

    Ok((start_time, end_time))
}

fn merge_events(events: &mut Vec<ScheduleEvent>) {
    // Sort events by all relevant fields for grouping
    events.sort_by_key(|event| (event.summary.clone(), event.note.clone(), event.location.clone(), event.booked, event.start_time));

    let merged_events: Vec<ScheduleEvent> = events
        .iter()
        .chunk_by(|event| (event.summary.clone(), event.note.clone(), event.location.clone(), event.booked))
        .into_iter()
        .flat_map(|(_, group)| {
            let mut merged_events: Vec<ScheduleEvent> = Vec::new();
            let mut iter = group.peekable();

            while let Some(current) = iter.next() {
                let mut merged_event = current.clone();
                while let Some(next) = iter.peek() {
                     if merged_event.can_merge(next) {
                         merged_event.end_time = next.end_time;
                         iter.next(); // Consume the next event since it's merged
                     } else {
                         break;
                    }
                }
                merged_events.push(merged_event);
           }
           merged_events
        })
        .collect();

    *events = merged_events;
    events.sort_by_key(|event| event.start_time); // Sort by start time after merging
}

fn split_overlapping_events(events: &mut Vec<ScheduleEvent>, new_event: ScheduleEvent, timezone: &Tz) -> bool {
    let mut overlaps_exist = false;
    let mut new_events = Vec::new();
    let original_events = events.clone();

    for existing_event in events.drain(..) {
        if new_event.start_time < existing_event.end_time && new_event.end_time > existing_event.start_time {
            // Overlap: Split existing event
            overlaps_exist = true;

            if new_event.start_time > existing_event.start_time {
                // Add the portion of the existing event before the new event
                let before_event = ScheduleEvent {
                    id: Uuid::new_v4().to_string(),
                    start_time: existing_event.start_time,
                    end_time: new_event.start_time,
                    summary: existing_event.summary.clone(),
                    note: existing_event.note.clone(),
                    location: existing_event.location.clone(),
                    booked: existing_event.booked,
                };
                new_events.push(before_event);

            }

            if new_event.end_time < existing_event.end_time {
                // Add portion of the existing event after the new event
                let after_event = ScheduleEvent {
                    id: Uuid::new_v4().to_string(),
                    start_time: new_event.end_time,
                    end_time: existing_event.end_time,
                    summary: existing_event.summary.clone(),
                    note: existing_event.note.clone(),
                    location: existing_event.location.clone(),
                    booked: existing_event.booked,
                };

                new_events.push(after_event);

            }

        } else {
             // No overlap: Keep existing event
            new_events.push(existing_event);
        }
    }

    // Add the new event
    new_events.push(new_event);
    // Replace original events with modified ones
    *events = new_events;
    // Sort events by start time
    events.sort_by(|a, b| a.start_time.cmp(&b.start_time));
    merge_events(events); // Merge after splitting and adding

    if overlaps_exist {
        print_event_diff(&original_events, events, &timezone);
    }
    overlaps_exist
}

fn print_event_diff(before: &[ScheduleEvent], after: &[ScheduleEvent], timezone: &Tz) {
    println!("{}", "Changes to existing events:".yellow().bold());

    let before_map: HashMap<&str, &ScheduleEvent> = before.iter().map(|e| (e.id.as_str(), e)).collect();
    let after_map: HashMap<&str, &ScheduleEvent> = after.iter().map(|e| (e.id.as_str(), e)).collect();

    // Deleted events
    for id in before_map.keys() {
        if !after_map.contains_key(id) {
            println!("- {}", format_event_for_diff(before_map[id], timezone).red());
        }
    }

    // Added or modified events
    for id in after_map.keys() {
        if !before_map.contains_key(id) {
            println!("+ {}", format_event_for_diff(after_map[id], timezone).green());
        } else if before_map[id] != after_map[id] { // Direct comparison
            let before_event = before_map[id];
            let after_event = after_map[id];
            println!("~ {}", format_event_change_for_diff(before_event, after_event, timezone).yellow());
        }
    }
    println!();
}

fn format_event_for_diff(event: &ScheduleEvent, timezone: &Tz) -> String {
    let start_time = event.start_time.with_timezone(timezone);
    let end_time = event.end_time.with_timezone(timezone);
    format!(
        "{} - {} {} ({}) {} {}",
        start_time.format("%Y-%m-%d %H:%M"),
        end_time.format("%H:%M"),
        event.summary,
        event.id,
        event.note.as_deref().unwrap_or_default(),
        event.location.as_deref().unwrap_or_default()

    )
}

fn format_event_change_for_diff(before: &ScheduleEvent, after: &ScheduleEvent, timezone: &Tz) -> String {
    let mut changes = Vec::new();

    if before.start_time.date_naive() != after.start_time.date_naive() {
        changes.push(format!(
            "~  date    : {} → {}",
            before.start_time.with_timezone(timezone).format("%Y-%m-%d"),
            after.start_time.with_timezone(timezone).format("%Y-%m-%d")
        ).yellow().to_string());
    }
    if before.start_time.time() != after.start_time.time() {
        changes.push(format!(
            "~  start   : {} → {}",
            before.start_time.with_timezone(timezone).format("%H:%M"),
            after.start_time.with_timezone(timezone).format("%H:%M")
        ).yellow().to_string());
    }
    if before.end_time.time() != after.end_time.time() {
        changes.push(format!(
            "~  end     : {} → {}",
            before.end_time.with_timezone(timezone).format("%H:%M"),
            after.end_time.with_timezone(timezone).format("%H:%M")
        ).yellow().to_string());
    }
    if before.note != after.note {
        let before_note = before.note.as_deref().unwrap_or_default();
        let after_note = after.note.as_deref().unwrap_or_default();

        let change_string = match (&before.note, &after.note) {
            (Some(_), None) => format!("-  note    : {}", before_note).red().to_string(),
            (None, Some(_)) => format!("+  note    : {}", after_note).green().to_string(),
            _ => format!("~  note    : {} → {}", before_note, after_note).yellow().to_string(),
        };
        changes.push(change_string);
    }
    if before.location != after.location {
        let before_location = before.location.as_deref().unwrap_or_default();
        let after_location = after.location.as_deref().unwrap_or_default();
        let change_string = match (&before.location, &after.location) {
            (Some(_), None) => format!("-  location: {}", before_location).red().to_string(),
            (None, Some(_)) => format!("+  location: {}", after_location).green().to_string(),
            _ => format!("~  location: {} → {}", before_location, after_location).yellow().to_string(),
        };
        changes.push(change_string);
    }
    if before.booked != after.booked {
        changes.push(format!("~  booked  : {} → {}", before.booked, after.booked).yellow().to_string());
    }

    format!(
        "{} ({})\n{}",
        after.summary,
        after.id,
        changes.join("\n")
    )
}

fn load_events(file_path: &PathBuf) -> Result<Vec<ScheduleEvent>, Error> {
    match File::open(file_path) {
        Ok(file) => serde_json::from_reader(file).map_err(|e| {
            Error::new(
                ErrorKind::InvalidData,
                format!("Failed to parse schedule file: {}", e),
            )
        }),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

fn save_events(file_path: &PathBuf, events: &[ScheduleEvent]) -> Result<(), Error> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(file_path)?;

    file.seek(SeekFrom::Start(0))?;
    file.set_len(0)?;

    serde_json::to_writer(&file, events)?;
    Ok(())
}

fn generate_ics(file_path: &PathBuf, events: &[ScheduleEvent], export_notes: bool) -> Result<(), Error> {
    let mut calendar = ICalendar::new("2.0", "-//plantrack//plantrack version 1.0//EN");

    let now = Utc::now().with_second(0).unwrap().with_nanosecond(0).unwrap();
    let past_cutoff = now - Duration::days(7); // Include past 7 days in the export

    let mut exported_events_count = 0;

    for event in events {
        // Export future events and events within the past time window
        if event.start_time >= past_cutoff {
            let mut ics_event = Event::new(event.id.clone(), event.start_time.format("%Y%m%dT%H%M%SZ").to_string());
            let (project, _) = event.summary.split_once(':').ok_or_else(|| Error::new(ErrorKind::InvalidInput, "Invalid project:task format"))?;
            ics_event.push(Summary::new(project.trim()));
            ics_event.push(DtStart::new(event.start_time.format("%Y%m%dT%H%M%SZ").to_string()));
            ics_event.push(DtEnd::new(event.end_time.format("%Y%m%dT%H%M%SZ").to_string()));

            ics_event.push(if event.booked { Status::new("CONFIRMED") } else { Status::new("TENTATIVE") });

            if export_notes {
                if let Some(note) = &event.note {
                    ics_event.push(Description::new(note.clone()));
                }
            }
            if let Some(loc) = &event.location {
                ics_event.push(Location::new(loc.clone()));
            }

            calendar.add_event(ics_event);
            exported_events_count += 1;
        }
    }

    calendar.save_file(file_path)?;
    println!("{} events exported to {}", exported_events_count, file_path.display());
    Ok(())
}

fn print_events_grouped_by_day(events: &[ScheduleEvent], timezone: &Tz, days: u32, date_str: Option<String>, past: bool) {
    let now = if let Some(date_str) = date_str {
        match NaiveDate::parse_from_str(&date_str, "%Y-%m-%d") {
            Ok(date) => timezone.from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap()).unwrap(),
            Err(_) => {
                println!("{}", "Invalid date format. Using today.".yellow());
                Utc::now().with_second(0).unwrap().with_nanosecond(0).unwrap().with_timezone(timezone)
            }
        }
    } else {
        Utc::now().with_second(0).unwrap().with_nanosecond(0).unwrap().with_timezone(timezone)
    };
    let realnow = Utc::now().with_second(0).unwrap().with_nanosecond(0).unwrap().with_timezone(timezone);
    let day_range = if past {
        -(days as i64)..=-1
    } else {
        0..=(days as i64)
    };

    for day_offset in day_range {
        let current_date = now.date_naive() + Duration::days(day_offset as i64);
        let date_str = current_date.format("%Y-%m-%d - %a").to_string();
        let date_str = if current_date == realnow.date_naive() {
            date_str.bright_yellow().bold().to_string()
        } else {
            date_str.bright_blue().bold().to_string()
        };
        println!("{}", date_str);

        let events_for_day: Vec<&ScheduleEvent> = events
            .iter()
            .filter(|event| {
                let event_start_date = event.start_time.with_timezone(timezone).date_naive();
                 event_start_date == current_date
            })
            .collect();

        print_day_travel(&events_for_day);
        if events_for_day.is_empty() {
            println!("    {}", "No events".italic());
        } else {
            let mut last_end_time: Option<DateTime<Tz>> = None;
            for event in events_for_day {
                let start_time_local = event.start_time.with_timezone(timezone);

                if let Some(last_et) = last_end_time {
                    let free_time = start_time_local - last_et;
                    if free_time > Duration::zero() {
                        let now = Utc::now().with_second(0).unwrap().with_nanosecond(0).unwrap().with_timezone(timezone);
                        if last_et <= now && now < start_time_local {
                            let from_last_event = now - last_et;
                            let til_next_event = start_time_local - now;
                            println!("       {}                  {}",
                                format_duration(from_last_event, true).bright_yellow().italic(),
                                format!("⋮").bright_yellow());
                            println!("  {}                        {} {}",
                                "› ...".bright_yellow(),
                                format_duration(free_time, true).bright_yellow(),
                                "free".bright_yellow());
                            println!("       {}                  {}",
                                format_duration(til_next_event, true).bright_yellow().italic(),
                                format!("⋮").bright_yellow());
                        } else {
                            println!("                               {}", format!("⋮").bright_green());
                            println!("                               {} {}", format_duration(free_time, true).bright_green(), "free".bright_green());
                            println!("                               {}", format!("⋮").bright_green());
                        }
                    }
                }

                print_event(event, timezone);
                last_end_time = Some(event.end_time.with_timezone(timezone));
                // print_event(event, timezone);
            }
        }
        println!();
    }
}

// Print travel information
fn print_day_travel(events_for_day: &[&ScheduleEvent]) {
    if !events_for_day.is_empty() {

        let mut travel_info: Vec<String> = Vec::new();
        let mut last_location: Option<String> = None;
        for event in events_for_day {
            if let Some(location) = &event.location {
                if last_location.as_ref() != Some(location) {
                    if let Some(last_loc) = last_location {
                        travel_info.push(last_loc);
                    }
                    last_location = Some(location.clone());
                }
            }
        }
        if let Some(last_loc) = last_location {
            travel_info.push(last_loc);
        }
        if travel_info.len() > 1 {
            println!("           {}", format!("↳ ✈: {}", travel_info.join(" → ")).bright_blue().italic());
        } else if travel_info.len() == 1 {
            println!("           {}", format!("↳ ⌂: {}", travel_info.join(" → ")).bright_blue().italic());
        }
    }
}

fn print_event(event: &ScheduleEvent, timezone: &Tz) {
    let start_time_local = event.start_time.with_timezone(timezone);
    let end_time_local = event.end_time.with_timezone(timezone);
    let duration = end_time_local - start_time_local; // Calculate duration in local time
    let now = Utc::now().with_second(0).unwrap().with_nanosecond(0).unwrap().with_timezone(timezone);
    let booked_str = if event.booked {
        "✔".green()
    } else {
        if end_time_local < now { "✗".red() } else { "≈".blue() }
    };
    let (project, task) = event.summary.split_once(':').unwrap_or(("", &event.summary));

    let event_str = format!(
        "{:02}:{:02} - {:02}:{:02} ({:02}:{:02}h) [{}] {}:{} ({})",
        start_time_local.hour(),
        start_time_local.minute(),
        end_time_local.hour(),
        end_time_local.minute(),
        duration.num_hours(),
        duration.num_minutes() % 60,
        booked_str,
        project.bold().blue(),
        task,
        event.id.italic().dimmed(),
    );

    if start_time_local <= now && now < end_time_local {
        println!("  {}", format!("› {}", event_str).yellow()); // Highlight current event

        // // Percentage bar
        // let elapsed_duration = now - start_time_local;
        // let percentage = (elapsed_duration.num_minutes() as f64 / duration.num_minutes() as f64) * 100.0;
        // let bar_width = 20; // Adjust as needed
        // let filled_width = (percentage / 100.0 * bar_width as f64) as usize;
        // let bar = format!(
        //     "[{}{}] {:.1}%",
        //     "#".repeat(filled_width).yellow(),
        //     " ".repeat(bar_width - filled_width),
        //     percentage
        // );
        // println!("    {}", bar);
    } else {
        println!("    {}", event_str);
    }

    if let Some(note) = &event.note {
        println!("                               {}", format!("↳ ✎: {}", note).bright_blue());
    }
    if let Some(location) = &event.location {
        println!("                               {}", format!("↳ ⌂: {}", location).bright_blue());
    }
}

fn format_duration(duration: Duration, human: bool) -> String {
    if human {
        let hours = duration.num_hours();
        let minutes = duration.num_minutes() % 60;
        format!("{:02}:{:02}h", hours, minutes)
    } else {
        let hours = duration.num_minutes() as f64 / 60.0;
        format!("{:.2}", hours)
    }
}

fn list_events(events: &[ScheduleEvent],past_days: u32, future_days: u32, date_str: Option<String>, timezone: &Tz, summary: bool) {
    if events.is_empty() {
        println!("{}", "No events found".yellow());
        return;
    }

    let now = if let Some(date_str) = date_str.clone() {
        match NaiveDate::parse_from_str(&date_str, "%Y-%m-%d") {
            Ok(date) => timezone.from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap()).unwrap(),
            Err(_) => {
                println!("{}", "Invalid date format. Using today.".yellow());
                Utc::now().with_second(0).unwrap().with_nanosecond(0).unwrap().with_timezone(timezone)
            }
        }
    } else {
        Utc::now().with_second(0).unwrap().with_nanosecond(0).unwrap().with_timezone(timezone)
    };

    // let time_window_start = (now - Duration::days(days as i64)).with_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap()).unwrap();
    // let time_window_end = (now + Duration::days(days as i64)).with_time(NaiveTime::from_hms_opt(23, 59, 59).unwrap()).unwrap();

    // let filtered_events: Vec<&ScheduleEvent> = events
    //     .iter()
    //     .filter(|event| event.start_time <= time_window_end && event.end_time >= time_window_start)
    //     .collect();

    // println!("Showing events within {} from {} in timezone: {}", format!("-{} and +{} days", past_days, future_days).yellow().bold(), now.date_naive(), timezone.name().green().bold());
    let start_date = now - Duration::days(past_days as i64);
    let end_date = now + Duration::days(future_days as i64);

    println!(
        "Showing events from {} to {} in timezone: {}",
        format!("{}", start_date.format("%Y-%m-%d")).bright_cyan().bold(),
        format!("{}", end_date.format("%Y-%m-%d")).bright_cyan().bold(),
        timezone.name().bright_green().bold()
    );
    if summary {
        let start_date = now - Duration::days(past_days as i64);
        let end_date = now + Duration::days(future_days as i64);

        let events_in_range: Vec<&ScheduleEvent> = events
            .iter()
            .filter(|event| {
                event.start_time >= start_date.with_timezone(&Utc) && event.end_time <= end_date.with_timezone(&Utc)
            })
            .collect();
        
        let mut project_summary: HashMap<String, Duration> = HashMap::new();

        for event in events_in_range.clone() {
            let (project, _) = event.summary.split_once(':').unwrap_or(("", &event.summary));
            let duration = event.end_time - event.start_time;
            *project_summary.entry(project.to_string()).or_insert(Duration::zero()) += duration;
        }

        let mut travel_info: Vec<String> = Vec::new();
        let mut last_location: Option<String> = None;
        for event in events_in_range {
            if let Some(location) = &event.location {
                if last_location.as_ref() != Some(location) {
                    if let Some(last_loc) = last_location {
                        travel_info.push(last_loc);
                    }
                    last_location = Some(location.clone());
                }
            }
        }
        if let Some(last_loc) = last_location {
            travel_info.push(last_loc);
        }
        if travel_info.len() >= 1 {
            println!("\n{} ({})", "Summary:".bright_yellow().bold(), travel_info.join(" → ").bright_blue().italic());
        } else {
            println!("\n{}", "Summary:".bright_yellow().bold());
        }
        for (project, duration) in project_summary {
            println!("  {}: {}", project.bright_blue(), format_duration(duration, true));
        }
    }
    println!("\n");
    print_events_grouped_by_day(events, timezone, past_days, date_str.clone(), true);
    print_events_grouped_by_day(events, timezone, future_days, date_str, false);
    // print_events_grouped_by_day(&filtered_events, timezone);
}

fn generate_report(events: &[ScheduleEvent], project: &str, timezone: &Tz, month: Option<u32>, year: Option<i32>, target_time: Option<f64>) {
    let now = Utc::now().with_second(0).unwrap().with_nanosecond(0).unwrap().with_timezone(timezone);
    let current_year = now.year();
    let current_month = now.month();

    let year = year.unwrap_or(current_year);
    let month = month.unwrap_or(current_month);

    println!("+------------------------");
    println!("|{}", format!("Report for Project: {}", project).bright_blue().bold());
    println!("|{}", format!("Month/Year: {}/{}", month, year).bright_yellow().bold());
    println!("|{}", format!("Timezone: {}", timezone.name()).yellow());
    println!("{}\n", "+---------------".dimmed()); // Use dimmed for separator
    let project_events: Vec<&ScheduleEvent> = events
        .iter()
        .filter(|event| {
            let event_time = event.start_time.with_timezone(timezone);
            event.summary.starts_with(&format!("{}:", project)) &&
            event_time.year() == year &&
            event_time.month() == month
        })
        .collect();

    if project_events.is_empty() {
        println!("{}", format!("No events found for project {} in {}/{}", project, month, year).yellow());
        return;
    }

    let mut tasks: HashMap<&str, Vec<&ScheduleEvent>> = HashMap::new();

    let mut planned_time = Duration::zero();
    let mut booked_time = Duration::zero();
    for event in &project_events {
        let duration = event.end_time - event.start_time;
        if event.booked {
            booked_time = booked_time + duration;
        } else {
            planned_time = planned_time + duration;
        }
        let task = event.summary.split_once(':').unwrap_or(("", "")).1;
        tasks.entry(task).or_insert(Vec::new()).push(event);
    }


    let mut sum_total_duration = Duration::zero();
    for (task, task_events) in tasks {
        let total_duration: Duration = task_events.iter().map(|event| event.end_time - event.start_time).sum();
        sum_total_duration = sum_total_duration + total_duration;

        println!("{}", format!("Task: {}", task).green().bold());
        println!("  {}", format!("Total Time: {}", format_duration(total_duration, false)).bright_white());

        for event in task_events {
            let duration = event.end_time - event.start_time;
            let booked = if event.booked {
                "[✔]".green()
            } else {
                if event.end_time.with_timezone(timezone) < Utc::now().with_second(0).unwrap().with_nanosecond(0).unwrap().with_timezone(timezone) {
                    "[✗]".red()
                } else {
                    "[≈]".blue()
                }
            };
            println!(
                "    {} - {} ({}) {} {} ({})",
                event.start_time.with_timezone(timezone).format("%Y-%m-%d %H:%M"),
                event.end_time.with_timezone(timezone).format("%H:%M"),
                format_duration(duration, false),
                booked,
                event.note.as_deref().unwrap_or_default(),
                event.id.italic().dimmed(),
            );
         }
         println!();
    }
    println!("{}", "Summary".yellow().bold()); // Clearer section header
    println!("  {}", format!("Total Time  : {}", format_duration(sum_total_duration, false)).bright_white().bold());
    println!("  {}", format!("Planned Time: {}", format_duration(planned_time, false)).bright_blue());
    println!("  {}", format!("Booked Time : {}", format_duration(booked_time, false)).bright_green());

    if let Some(target) = target_time {
        let target_duration = Duration::minutes((target * 60.0) as i64);
        let diff = sum_total_duration - target_duration;

        let diff_hours = diff.num_hours();
        let diff_minutes = diff.num_minutes() % 60;

        let diff_str = if diff > Duration::zero() {
            format!("Overrun     : {diff_hours}h {diff_minutes}m").green()
        } else {
            format!("Underrun    : {}h {}m", -diff_hours, -diff_minutes).red()
        };

        let percentage_diff = if target_duration != Duration::zero() {
            (diff.num_minutes() as f64 / target_duration.num_minutes() as f64) * 100.0
        } else {
            0.0 // Avoid division by zero if target is zero.
        };

        let target_hours = target_duration.num_hours();
        let target_minutes = target_duration.num_minutes() % 60;
        let target_str = format!("{target_hours}h {target_minutes}m");
        // println!("  Time difference: {} of {} ({:.1}%)", diff_str, target_str.blue(), percentage_diff);
        println!("  {}", format!("Target time : {}", target_str).bright_cyan());
        println!("  {}", format!("{} ({:.1}%)", diff_str, percentage_diff).bright_white());
    }
    println!("");
}

fn cleanup_events(events: &mut Vec<ScheduleEvent>, days: u32) {
    let cutoff_date = Utc::now().with_second(0).unwrap().with_nanosecond(0).unwrap() - Duration::days(days as i64);
    events.retain(|event| event.end_time > cutoff_date);
}

fn is_slot_free(events: &[ScheduleEvent], start_time: DateTime<Utc>, end_time: DateTime<Utc>) -> Result<bool, Vec<ScheduleEvent>> {
    let conflicting_events: Vec<ScheduleEvent> = events
        .iter()
        .filter(|event| start_time < event.end_time && end_time > event.start_time)
        .cloned()
        .collect();

    if conflicting_events.is_empty() {
        Ok(true)
    } else {
        Err(conflicting_events)
    }
}

fn delete_event(events: &mut Vec<ScheduleEvent>, id: &str, timespan: Option<String>, rounding: u32, timezone: &Tz) -> Result<bool, Error> {
    let event_index = events.iter().position(|event| event.id == id);

    if let Some(index) = event_index {
        if let Some(timespan_str) = timespan {
            let original_event = events[index].clone();
            let event_date = original_event.start_time.date_naive().format("%Y-%m-%d");
            let (start_remove, end_remove) = parse_datetime_range(&timespan_str, Some(event_date.to_string().as_str()), rounding, timezone)?;


            if start_remove >= original_event.end_time || end_remove <= original_event.start_time {
                println!("{}", "Specified timespan does not overlap with the event.".yellow());
                return Ok(false); // No changes made
            }


            let mut modified_events = Vec::new();

            if start_remove > original_event.start_time {
                modified_events.push(ScheduleEvent {
                    id: Uuid::new_v4().to_string(),
                    start_time: original_event.start_time,
                    end_time: start_remove,
                    summary: original_event.summary.clone(),
                    note: original_event.note.clone(),
                    location: original_event.location.clone(),
                    booked: original_event.booked,
                });
            }

            if end_remove < original_event.end_time {
                modified_events.push(ScheduleEvent {
                    id: Uuid::new_v4().to_string(),
                    start_time: end_remove,
                    end_time: original_event.end_time,
                    summary: original_event.summary.clone(),
                    note: original_event.note.clone(),
                    location: original_event.location.clone(),
                    booked: original_event.booked,
                });
            }

            print_event_diff(&[original_event], &modified_events, &timezone);

            let confirmed = Confirm::with_theme(&ColorfulTheme::default())
                .with_prompt("Apply these changes?")
                .interact();

            if confirmed.is_err() || !confirmed.unwrap() {
                println!("{}", "Changes not applied".yellow());
                return Ok(false); // No changes made
            }


            events.remove(index);
            events.extend(modified_events);
            events.sort_by_key(|e| e.start_time);
            merge_events(events);


        } else {
            let original_event = &events[index];

            println!("{}", "Deleting the following event:".yellow().bold());
            println!("- {}", format_event_for_diff(&original_event, &timezone).red());

            let confirmed = Confirm::with_theme(&ColorfulTheme::default())
                .with_prompt("Delete this event?")
                .interact();


            if confirmed.is_err() || !confirmed.unwrap() {
                println!("{}", "Event not deleted".yellow());
                return Ok(false);
            }

            events.remove(index);

        }
        Ok(true) // Changes made
    } else {
        println!("Event with ID {} not found", id.green().bold());
        Ok(false) // No changes made
    }
}

fn find_next_event_time(events: &[ScheduleEvent], project_task: &str, duration_minutes: u32, now: DateTime<Utc>) -> Result<(DateTime<Utc>, DateTime<Utc>), Error> {
    let duration = Duration::minutes(duration_minutes as i64);
    let target_events: Vec<&ScheduleEvent> = events
        .iter()
        .filter(|event| event.summary == project_task && event.start_time >= now && event.end_time > event.start_time + duration)
        .collect();

    if let Some(target_event) = target_events.first() {
        Ok((target_event.start_time, target_event.start_time + duration))
    } else {
        Err(Error::new(ErrorKind::NotFound, format!("No upcoming event found with project:task '{}' and length of {} minutes", project_task, duration.num_minutes())))
    }
}


fn find_free_slot(
    events: &[ScheduleEvent],
    timespan: &str,
    date_str: Option<&str>,
    duration_minutes: u32,
    rounding: u32,
    timezone: &Tz,
) -> Result<(DateTime<Utc>, DateTime<Utc>), Error> {
    let (start_time, end_time) = parse_datetime_range(timespan, date_str, rounding, timezone)?;

    let mut current_time = start_time;
    let duration = Duration::minutes(duration_minutes as i64);
    let now = Utc::now().with_second(0).unwrap().with_nanosecond(0).unwrap();

    if current_time < now {
        current_time = now;
        let rounded_time = round_time_to_interval(current_time.naive_utc().time(), rounding, true);
        current_time = current_time.with_time(rounded_time).unwrap();
    }
    
    while current_time + duration <= end_time {
        let proposed_end_time = current_time + duration;
        if is_slot_free(events, current_time, proposed_end_time).is_ok() {
            return Ok((current_time, proposed_end_time));
        }
        current_time = current_time + duration;
    }

    Err(Error::new(ErrorKind::Other, "Could not find free slot."))
}

fn round_duration_up(duration: Duration, interval: u32) -> Duration {
    let minutes = duration.num_minutes();
    let remainder = minutes % interval as i64;
    if remainder > 0 {
        duration + Duration::minutes((interval as i64 - remainder) as i64)
    } else {
        duration
    }
}

fn main() -> Result<(), Error> {
    let args = Args::parse();

    let config_path = match args.config_file {
        Some(path) => {
            if path.is_absolute() {
                path
            } else {
                std::env::current_dir()?.join(path) // Make relative path absolute
            }
        },
        None => {
            let xdg_dirs = BaseDirectories::with_prefix(APP_NAME)?;
            let config_dir = xdg_dirs.get_config_home();
            config_dir.join(DEFAULT_CONFIG_FILE) // Otherwise use the XDG directory
        }
    };
    let config = Config::load(&config_path)?; // Pass the resolved path to Config::load

    let schedule_file_path = config.schedule_file;
    let ics_file_path = config.ics_file;

    let mut events: Vec<ScheduleEvent> = load_events(&schedule_file_path)?;

    let timezone: Tz = match args.timezone.as_deref() { // CLI argument has highest priority
        Some(tz_str) => tz_str.parse().map_err(|_| {
            Error::new(ErrorKind::InvalidInput, "Invalid timezone format. Use IANA format (e.g., America/New_York).")
        })?,
        None => match var("TZ").ok().and_then(|tz_env| tz_env.parse().ok()) { // Then check environment variable
            Some(tz) => tz,
            None => match iana_time_zone::get_timezone().unwrap().parse().ok() {
                Some(tz) => tz,
                None => config.timezone.as_deref().and_then(|tz_config| tz_config.parse().ok()).unwrap_or(Tz::UTC) // Then config, finally UTC
            }
        },
    };

    let export_notes = config.export_notes.unwrap_or(true); // Read export_notes from config, defaulting to true

    let rounding = args.rounding.or(config.rounding).unwrap_or(15); // Rounding handling: CLI > Config > Default (15)

    match args.command {
        Commands::Add {
            project_task,
            timespan,
            date,
            note,
            location,
            booked,
        } => {
            let (start_time, end_time) = parse_datetime_range(&timespan, date.as_deref(), rounding, &timezone)?;
            let (project, task) = project_task
                .split_once(':')
                .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "Invalid project:task format"))?;
            let summary = format!("{}:{}", project.trim(), task.trim());

            let event = ScheduleEvent {
                id: Uuid::new_v4().to_string(),
                start_time,
                end_time,
                summary,
                note,
                location,
                booked,
            };

            let overlaps = split_overlapping_events(&mut events, event.clone(), &timezone);
            if !overlaps {
                println!("{}", "New event:".yellow().bold());
                println!("+ {}", format_event_for_diff(&event,&timezone).green());
            }

            let confirmed = Confirm::with_theme(&ColorfulTheme::default())
                .with_prompt(if overlaps { "Overlapping events found. Add anyway?" } else { "Add this event?" })
                .interact();

            if confirmed.is_err() || !confirmed.unwrap() {
                println!("{}", "Event not added".yellow());
                return Ok(());
            }
            // if overlaps {
            //     let confirmed = Confirm::with_theme(&ColorfulTheme::default())
            //         .with_prompt("Overlapping events found. Overwrite?")
            //         .interact();

            //     if confirmed.is_err() || !confirmed.unwrap() {
            //         println!("{}", "Event not added".yellow());
            //         return Ok(()); // Exit early if the user cancels or an error occurs
            //     }
            // }
            save_events(&schedule_file_path, &events)?;
            generate_ics(&ics_file_path, &events, export_notes)?;
            println!("{}", "Event added".green());
        }
        Commands::Quickadd { project_task, minutes, note, location, forward } => {
            let now = Utc::now().with_second(0).unwrap().with_nanosecond(0).unwrap();
            let duration_minutes = round_duration_up(Duration::minutes(minutes.unwrap_or(rounding) as i64), rounding);

            let (start_time, end_time) = if forward {
                let start_time = round_time_to_interval(now.naive_utc().time(), rounding, false);
                let start_time = now.with_time(start_time).unwrap();
                let end_time = start_time + duration_minutes;
                (start_time, end_time)
            } else {
                let end_time = round_time_to_interval(now.naive_utc().time(), rounding, true);
                let end_time = now.with_time(end_time).unwrap();
                let start_time = end_time - duration_minutes;
                (start_time, end_time)
            };

            let (project, task) = project_task
                .split_once(':')
                .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "Invalid project:task format"))?;
            let summary = format!("{}:{}", project.trim(), task.trim());

            let event = ScheduleEvent {
                id: Uuid::new_v4().to_string(),
                start_time,
                end_time,
                summary,
                note,
                location,
                booked: true,
            };

            let overlaps = split_overlapping_events(&mut events, event.clone(), &timezone);
            if !overlaps {
                println!("{}", "New event:".yellow().bold());
                println!("+ {}", format_event_for_diff(&event, &timezone).green());
            }

            let confirmed = Confirm::with_theme(&ColorfulTheme::default())
                .with_prompt(if overlaps { "Overlapping events found. Add anyway?" } else { "Add this event?" })
                .interact();

            if confirmed.is_err() || !confirmed.unwrap() {
                println!("{}", "Event not added".yellow());
                return Ok(());
            }
            // if overlaps {
            //     let confirmed = Confirm::with_theme(&ColorfulTheme::default())
            //         .with_prompt("Overlapping events found. Overwrite?")
            //         .interact();

            //     if confirmed.is_err() || !confirmed.unwrap() {
            //         println!("{}", "Event not added".yellow());
            //         return Ok(()); // Exit early if the user cancels
            //     }
            // }
            // split_overlapping_events(&mut events, event.clone());
            // merge_events(&mut events);

            save_events(&schedule_file_path, &events)?;
            generate_ics(&ics_file_path, &events, export_notes)?;
            println!("{}", "Event added".green());
        }
        Commands::Todo { project_task, minutes, in_project_task, date, timespan, note, location } => {
            let rounding_interval = rounding;
            let duration_minutes = minutes.unwrap_or(rounding_interval * 2);
            let now = Utc::now().with_second(0).unwrap().with_nanosecond(0).unwrap();

            let (start_time, end_time) = if let Some(in_proj_task) = in_project_task {
                find_next_event_time(&events, &in_proj_task, duration_minutes, now)?
            } else {
                match find_free_slot(&events, &timespan, date.as_deref(), duration_minutes, rounding, &timezone) {
                    Ok(slot) => slot,
                    Err(e) => {
                        println!("{}", e); // Indicate why no free slot could be found
                        return Ok(()); // Do not attempt to create a todo if no free slot is available
                    }

                }
            };

            let (project, task) = project_task.split_once(':').ok_or(Error::new(ErrorKind::InvalidInput, "Invalid project:task format"))?;
            let summary = format!("{}:{}", project.trim(), task.trim());

            let event = ScheduleEvent {
                id: Uuid::new_v4().to_string(),
                start_time,
                end_time,
                summary,
                note,
                location,
                booked: false,
            };

            println!("{} {}", "New todo on".yellow().bold(), format!("{}", event.start_time.date_naive()).yellow());
            print_event(&event, &timezone);

            if Confirm::with_theme(&ColorfulTheme::default())
                .with_prompt("Add this todo?")
                .interact().unwrap()
            {
                split_overlapping_events(&mut events, event, &timezone);
                save_events(&schedule_file_path, &events)?;
                generate_ics(&ics_file_path, &events, export_notes)?;
                println!("{}", "Todo added".green());
            } else {
                println!("{}", "Todo not added".yellow());
            }
        }
        Commands::List { past_days, future_days, date, summary } => list_events(&events, past_days, future_days, date, &timezone, summary),
        // Commands::List { days } => list_events(&events, days),
        Commands::Delete { id, timespan } => {
            if delete_event(&mut events, &id, timespan, rounding, &timezone)? {
                save_events(&schedule_file_path, &events)?;
                generate_ics(&ics_file_path, &events, export_notes)?;
            }
        }
        Commands::Report { project, month, year, target } => {
            generate_report(&events, &project, &timezone, month, year, target);
            // generate_ics(&ics_file_path, &events, export_notes)?;
        }
        Commands::Cleanup { days } => {
            cleanup_events(&mut events, days);
            save_events(&schedule_file_path, &events)?;
            println!("Cleaned up events older than {} days.", days);
            generate_ics(&ics_file_path, &events, export_notes)?;
        }
        Commands::Set { id, location, note, booked, timespan, date } => {
            let event_index = events.iter().position(|event| event.id == id).ok_or_else(|| {
                Error::new(ErrorKind::NotFound, format!("Event with ID {} not found", id))
            })?;

            let original_event = events[event_index].clone();

            let mut modified_event = original_event.clone();
            let mut modified = false;

            if let Some(location) = location {
                if location.is_empty() {
                    modified_event.location = None;
                } else {
                    modified_event.location = Some(location);
                }
                modified = true;
            }
            if let Some(note) = note {
                if note.is_empty() {
                    modified_event.note = None;
                } else {
                    modified_event.note = Some(note);
                }
                modified = true;
            }
            if let Some(booked) = booked {
                modified_event.booked = booked;
                modified = true;
            }

            // Simplified date/time handling
            let (new_start_time, new_end_time) = if let Some(date_str) = date {
                let timespan_str = timespan.unwrap_or_else(|| {
                    let start_time = original_event.start_time.with_timezone(&timezone).time();
                    let end_time = original_event.end_time.with_timezone(&timezone).time();
                    format!("{}-{}", start_time.format("%H:%M"), end_time.format("%H:%M"))
                });
                parse_datetime_range(&timespan_str, Some(&date_str), rounding, &timezone)?
            } else if let Some(timespan_str) = timespan {
                let date_str = original_event.start_time.with_timezone(&timezone).format("%Y-%m-%d").to_string();
                parse_datetime_range(&timespan_str, Some(&date_str), rounding, &timezone)?
            } else {
                (original_event.start_time, original_event.end_time)
            };
            if new_start_time != original_event.start_time || new_end_time != original_event.end_time {
                modified_event.start_time = new_start_time;
                modified_event.end_time = new_end_time;
                modified = true;
            }


            if modified {
                println!("{} {}", format!("Change event:").yellow().bold(), format_event_change_for_diff(&original_event, &modified_event, &timezone).yellow());
                if Confirm::with_theme(&ColorfulTheme::default())
                    .with_prompt("Apply these changes?")
                    .interact()
                    .unwrap()
                {
                    events.remove(event_index);
                    split_overlapping_events(&mut events, modified_event, &timezone);
                    save_events(&schedule_file_path, &events)?;
                    generate_ics(&ics_file_path, &events, export_notes)?;
                    println!("Event with ID {} modified", id.green().bold());
                } else {
                    println!("{}", "Changes not applied".yellow());
                }
            } else {
                println!("{}", "No changes specified for event".yellow());
            }
        }
        Commands::Current {} => {
            let now = Utc::now().with_second(0).unwrap().with_nanosecond(0).unwrap().with_timezone(&timezone);
            let current_event = events.iter().find(|event| {
                event.start_time <= now && now < event.end_time
            });

            match current_event {
                Some(event) => {
                    let (project, task) = event.summary.split_once(':').unwrap_or(("", &event.summary));
                    println!("🗓 {project}:{task}");
                }
                None => println!("🗓 No event"),
            }
         },
        Commands::Push {  } => {
            generate_ics(&ics_file_path, &events, export_notes)?;
            // Execute post-ICS command if configured
            if let Some(command_str) = &config.push_command {
                println!("Executing: {}", command_str);
                let status = Command::new("sh")
                    .arg("-c")
                    .arg(command_str)
                    .status()?;

                if !status.success() {
                    return Err(Error::new(
                        ErrorKind::Other,
                        format!("Command failed with exit code: {}", status),
                    ));
                }
            }
        }
        Commands::Free { timespan, date } => {
            let (start_time, end_time) = parse_datetime_range(&timespan, date.as_deref(), rounding, &timezone)?;

            let start_time_local = start_time.with_timezone(&timezone);
            let end_time_local = end_time.with_timezone(&timezone);

            match is_slot_free(&events, start_time, end_time) {
                Ok(true) => {
                    println!("{}", format!("\nSlot {} - {} on {} is free", start_time_local.format("%H:%M"), end_time_local.format("%H:%M"), start_time_local.format("%Y-%m-%d")).green());
                }
                Ok(false) => {
                    // This case is not possible anymore as is_slot_free either returns Ok(true) or Err(event)
                    unreachable!();
                },
                Err(conflicting_events) => {
                    if conflicting_events.iter().all(|e| !e.booked) {
                        println!("\n{}", format!("Slot {} - {} on {} is already planned", start_time_local.format("%H:%M"), end_time_local.format("%H:%M"), start_time_local.format("%Y-%m-%d")).yellow());
                    } else if conflicting_events.iter().any(|e| e.booked) {
                        println!("\n{}", format!("Slot {} - {} on {} is already booked", start_time_local.format("%H:%M"), end_time_local.format("%H:%M"), start_time_local.format("%Y-%m-%d")).red());
                    } else {
                        // Should not happen, but defaults to planned
                        println!("\n{}", format!("Slot {} - {} on {} is already planned", start_time_local.format("%H:%M"), end_time_local.format("%H:%M"), start_time_local.format("%Y-%m-%d")).yellow());
                    }
                    println!("{}", format!("Conflicting event:").bright_red());
                    for conflicting_event in &conflicting_events {
                        print_event(conflicting_event, &timezone);
                    }
                }
            }
            // Get all events for the specified date
            let date_naive = start_time_local.date_naive();
            let events_for_day: Vec<&ScheduleEvent> = events
                .iter()
                .filter(|event| event.start_time.with_timezone(&timezone).date_naive() == date_naive)
                .collect();

            // Print all events for the day regardless of conflicts
            if events_for_day.is_empty() {
                println!("\n{}", format!("No events on {}", date_naive.format("%Y-%m-%d")).bright_green());
            } else {
                println!("\n{}", format!("All events on {}:", date_naive.format("%Y-%m-%d")).blue().bold());
                print_day_travel(&events_for_day);
                let mut last_end_time: Option<DateTime<Tz>> = None;
                for event in events_for_day {
                    let start_time_local = event.start_time.with_timezone(&timezone);

                    if let Some(last_et) = last_end_time {
                        let free_time = start_time_local - last_et;
                        if free_time > Duration::zero() {
                            println!("                               {}", format!("⋮").bright_green());
                            println!("                               {} {}", format_duration(free_time, true).bright_green(), "free".bright_green());
                            println!("                               {}", format!("⋮").bright_green());
                        }
                    }

                    print_event(event, &timezone);
                    last_end_time = Some(event.end_time.with_timezone(&timezone));
                }
            }
        }
    }

    // generate_ics(&ics_file_path, &events, export_notes)?;
    Ok(())
}
