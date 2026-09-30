//! The Queue and ETA cells: how much work is ahead of a step, from the
//! `queued` gauge it reports, and when that queue empties at the pace it
//! has shrunk over the last couple of minutes. A group sums its steps'
//! queues and waits on the slowest of them.

use datalib_columns::Quantity;

use crate::{DagStepProgress, QueueTrend};

/// How long a running step may go without a metric moving before its
/// ETA says it has stalled rather than guessing.
const STALL_AFTER_SECS: i64 = 60;

/// The shortest look at the queue an estimate is drawn from. Less than
/// this and one page arriving swings it by an order of magnitude.
const MEASURE_SECS: f64 = 15.0;

const STALLED: &str = "stalled";
const GROWING: &str = "growing";
const FLAT: &str = "flat";
const MEASURING: &str = "measuring";

pub(super) fn grouped(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// The same words the ETA cell draws, for the hovers that explain it.
fn duration(secs: i64) -> String {
    if secs < 60 {
        format!("{secs} sec")
    } else if secs < 3600 {
        format!("{} min", (secs as f64 / 60.0).round() as i64)
    } else {
        format!("{:.1} h", secs as f64 / 3600.0)
    }
}

fn is_queued(name: &str) -> bool {
    name == "queued" || name.starts_with("queued{")
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cells {
    pub queue: Quantity,
    pub eta: Quantity,
}

fn blank(unit: &str) -> Quantity {
    Quantity {
        unit: unit.into(),
        ..Default::default()
    }
}

fn noted(note: &str, detail: String) -> Quantity {
    Quantity {
        value: None,
        unit: "seconds".into(),
        note: Some(note.into()),
        detail: Some(detail),
    }
}

/// A step's two cells from what it has reported this run. A step that
/// reports no `queued` series has neither; one that has finished keeps
/// its queue only while something is still in it.
pub fn step_cells(p: Option<&DagStepProgress>, running: bool) -> Cells {
    let Some(p) = p else {
        return Cells {
            queue: blank("count"),
            eta: blank("seconds"),
        };
    };
    let queued: Vec<(&String, i64)> = p
        .metrics
        .iter()
        .filter(|(n, _)| is_queued(n))
        .map(|(n, v)| (n, *v))
        .collect();
    let total: i64 = queued.iter().map(|(_, v)| v).sum();
    if queued.is_empty() || (!running && total == 0) {
        return Cells {
            queue: blank("count"),
            eta: blank("seconds"),
        };
    }
    let queue = Quantity {
        value: Some(total),
        unit: "count".into(),
        note: None,
        detail: Some(queue_detail(&queued)),
    };
    let eta = if running {
        eta(total, p.queue_trend, p.progress_age_secs, p.log_age_secs)
    } else {
        blank("seconds")
    };
    Cells { queue, eta }
}

fn queue_detail(queued: &[(&String, i64)]) -> String {
    if queued.len() == 1 {
        return "Work the step says is still ahead of it.".into();
    }
    let parts = queued
        .iter()
        .map(|(name, v)| {
            match name
                .strip_prefix("queued{from=")
                .and_then(|s| s.strip_suffix('}'))
            {
                Some(from) => format!("{} from {from}", grouped(*v)),
                None => format!("{} of its own", grouped(*v)),
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("Work still ahead of the step: {parts}.")
}

fn eta(
    total: i64,
    trend: Option<QueueTrend>,
    progress_age: Option<i64>,
    log_age: Option<i64>,
) -> Quantity {
    if total == 0 {
        return blank("seconds");
    }
    if let Some(age) = progress_age.filter(|a| *a >= STALL_AFTER_SECS) {
        let since = duration(age);
        let why = match log_age {
            Some(l) if l < STALL_AFTER_SECS => format!(
                "but it is still logging (last line {} ago) \u{2014} busy, not advancing",
                duration(l)
            ),
            Some(l) => format!("and it last logged {} ago \u{2014} silent", duration(l)),
            None => "and it has logged nothing".into(),
        };
        return noted(
            STALLED,
            format!(
                "No metric has moved for {since}, {why}. Double-click for the step's dashboard."
            ),
        );
    }
    let Some(trend) = trend.filter(|t| t.secs_ago >= MEASURE_SECS) else {
        return noted(
            MEASURING,
            format!(
                "The estimate is how fast the queue shrinks, and it takes {} seconds of \
                 watching.",
                MEASURE_SECS as i64
            ),
        );
    };
    let over = duration(trend.secs_ago.round() as i64);
    let drained = trend.queued_then - total;
    if drained <= 0 {
        let (note, how) = if drained < 0 {
            (GROWING, format!("grew by {}", grouped(-drained)))
        } else {
            (FLAT, "has not shrunk".to_string())
        };
        return noted(
            note,
            format!(
                "{} queued. Over the last {over} the queue {how}, so there is no pace to \
                 finish at yet.",
                grouped(total)
            ),
        );
    }
    let per_sec = drained as f64 / trend.secs_ago;
    let secs = (total as f64 / per_sec).ceil() as i64;
    Quantity {
        value: Some(secs),
        unit: "seconds".into(),
        note: None,
        detail: Some(format!(
            "{} queued, down {} over the last {over} (net of what arrived). At that pace the \
             queue is empty in about {}.",
            grouped(total),
            grouped(drained),
            duration(secs)
        )),
    }
}

/// A group's cells from its steps', each with the label it goes by.
/// The queue is the sum. The ETA waits on the slowest step, and a stall
/// anywhere outranks every estimate: a stuck step is the one thing the
/// group's figure must not hide.
pub fn group_cells(children: &[(&str, &Cells)]) -> Cells {
    let with_queue: Vec<(&str, i64)> = children
        .iter()
        .filter_map(|(l, c)| c.queue.value.map(|v| (*l, v)))
        .collect();
    let queue = if with_queue.is_empty() {
        blank("count")
    } else {
        Quantity {
            value: Some(with_queue.iter().map(|(_, v)| v).sum()),
            unit: "count".into(),
            note: None,
            detail: Some(
                with_queue
                    .iter()
                    .map(|(l, v)| format!("{l}: {}", grouped(*v)))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        }
    };
    let said = |(l, c): &(&str, &Cells)| {
        c.eta
            .detail
            .as_ref()
            .map(|d| format!("{l}: {d}"))
            .unwrap_or_default()
    };
    let noted_as = |note: &str| {
        children
            .iter()
            .find(|(_, c)| c.eta.note.as_deref() == Some(note))
    };
    let slowest = children
        .iter()
        .filter(|(_, c)| c.eta.value.is_some())
        .max_by_key(|(_, c)| c.eta.value);
    let eta = if let Some(stalled) = noted_as(STALLED) {
        noted(STALLED, said(stalled))
    } else if let Some(slowest) = slowest {
        Quantity {
            detail: Some(said(slowest)),
            ..slowest.1.eta.clone()
        }
    } else if let Some(c) = [GROWING, MEASURING, FLAT].into_iter().find_map(noted_as) {
        Quantity {
            detail: Some(said(c)),
            ..c.1.eta.clone()
        }
    } else {
        blank("seconds")
    };
    Cells { queue, eta }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(metrics: &[(&str, i64)], trend: Option<(i64, f64)>) -> DagStepProgress {
        DagStepProgress {
            msg: None,
            metrics: metrics.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            errors: 0,
            rates: Default::default(),
            progress_age_secs: Some(0),
            log_age_secs: Some(0),
            queue_trend: trend.map(|(queued_then, secs_ago)| QueueTrend {
                queued_then,
                secs_ago,
            }),
            updated_at_utc: String::new(),
        }
    }

    #[test]
    fn queue_sums_every_queued_series_and_ignores_the_rest() {
        let p = progress(
            &[("rows", 1234), ("queued", 5), ("queued{from=a/ingest}", 7)],
            None,
        );
        let c = step_cells(Some(&p), true);
        assert_eq!(c.queue.value, Some(12));
        assert!(c.queue.detail.unwrap().contains("7 from a/ingest"));
    }

    #[test]
    fn eta_is_the_queue_over_its_net_pace() {
        // 600 → 300 in 60s is 5/s; 300 left is 60s.
        let p = progress(&[("queued", 300)], Some((600, 60.0)));
        let c = step_cells(Some(&p), true);
        assert_eq!(c.eta.value, Some(60));
        assert_eq!(c.eta.note, None);
    }

    #[test]
    fn a_queue_that_is_not_shrinking_gets_a_word_not_a_figure() {
        let grew = step_cells(Some(&progress(&[("queued", 50)], Some((40, 60.0)))), true);
        assert_eq!(
            (grew.eta.value, grew.eta.note.as_deref()),
            (None, Some(GROWING))
        );
        let flat = step_cells(Some(&progress(&[("queued", 50)], Some((50, 60.0)))), true);
        assert_eq!(flat.eta.note.as_deref(), Some(FLAT));
        let young = step_cells(Some(&progress(&[("queued", 50)], Some((90, 5.0)))), true);
        assert_eq!(young.eta.note.as_deref(), Some(MEASURING));
    }

    #[test]
    fn a_stall_outranks_the_estimate() {
        let mut p = progress(&[("queued", 300)], Some((600, 60.0)));
        p.progress_age_secs = Some(90);
        p.log_age_secs = Some(5);
        let c = step_cells(Some(&p), true);
        assert_eq!(c.eta.note.as_deref(), Some(STALLED));
        assert!(c.eta.detail.unwrap().contains("busy, not advancing"));
    }

    #[test]
    fn a_finished_step_shows_nothing_once_its_queue_is_empty() {
        let p = progress(&[("queued", 0)], Some((10, 60.0)));
        assert_eq!(
            step_cells(Some(&p), false),
            Cells {
                queue: blank("count"),
                eta: blank("seconds"),
            }
        );
        // Work waiting on a step that has not started is still worth
        // showing, with no estimate.
        let waiting = step_cells(Some(&progress(&[("queued{from=a}", 4)], None)), false);
        assert_eq!(waiting.queue.value, Some(4));
        assert_eq!(waiting.eta, blank("seconds"));
    }

    #[test]
    fn a_group_sums_queues_and_waits_on_its_slowest_step() {
        let fast = step_cells(Some(&progress(&[("queued", 10)], Some((20, 60.0)))), true);
        let slow = step_cells(Some(&progress(&[("queued", 100)], Some((110, 60.0)))), true);
        let g = group_cells(&[("Ingest", &fast), ("Render", &slow)]);
        assert_eq!(g.queue.value, Some(110));
        assert_eq!(g.eta.value, slow.eta.value);
        assert!(g.eta.detail.unwrap().starts_with("Render: "));

        let mut stuck = progress(&[("queued", 1)], None);
        stuck.progress_age_secs = Some(300);
        let stuck = step_cells(Some(&stuck), true);
        let g = group_cells(&[("Ingest", &stuck), ("Render", &slow)]);
        assert_eq!(g.eta.note.as_deref(), Some(STALLED));
    }
}
