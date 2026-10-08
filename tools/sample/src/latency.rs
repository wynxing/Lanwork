use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ToolError;
use crate::session::p95_index_1based;

pub const MIN_VALID_SAMPLES: usize = 100;
pub const P95_RULE: &str = "升序第 ceil(0.95 × N) 个，从 1 起计";

const METRIC_HOT_RECALL: &str = "hot_recall";
const METRIC_RESULT: &str = "result";

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct RawLatency {
    pub metric: String,
    #[serde(default)]
    pub source: Option<String>,
    pub seq: u64,
    pub start_ns: u64,
    #[serde(default)]
    pub end_ns: Option<u64>,
    pub warmup: bool,
    pub superseded: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LatencyReport {
    pub p95_rule: &'static str,
    pub min_valid_samples: usize,
    pub groups: Vec<LatencyGroup>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LatencyGroup {
    pub metric: String,
    pub source: Option<String>,
    pub valid_count: usize,
    pub superseded_count: usize,
    pub warmup_excluded: usize,
    pub incomplete_count: usize,
    pub invalid_count: usize,
    pub sufficient: bool,
    /// `足够` 或 `不足`。
    pub sample_status: &'static str,
    pub p95_index: Option<usize>,
    pub p95_ns: Option<u64>,
    pub valid_samples: Vec<ValidSample>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ValidSample {
    pub seq: u64,
    pub start_ns: u64,
    pub end_ns: u64,
    pub duration_ns: u64,
}

pub fn parse_latency_jsonl(text: &str) -> Result<Vec<RawLatency>, ToolError> {
    let mut records = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            continue;
        }
        let record: RawLatency = serde_json::from_str(line)
            .map_err(|err| ToolError::new(format!("第 {} 行不是延迟记录：{err}", index + 1)))?;
        records.push(record);
    }
    Ok(records)
}

pub fn summarize(records: &[RawLatency]) -> Result<LatencyReport, ToolError> {
    for record in records {
        validate_record(record)?;
    }
    let mut groups: BTreeMap<(u8, String, String), GroupAccum> = BTreeMap::new();
    for record in records {
        let key = group_key(record);
        groups.entry(key).or_default().push(record);
    }
    let mut report_groups = Vec::with_capacity(groups.len());
    for ((_, metric, source), accum) in groups {
        report_groups.push(accum.finish(
            metric,
            if source.is_empty() {
                None
            } else {
                Some(source)
            },
        ));
    }
    Ok(LatencyReport {
        p95_rule: P95_RULE,
        min_valid_samples: MIN_VALID_SAMPLES,
        groups: report_groups,
    })
}

fn validate_record(record: &RawLatency) -> Result<(), ToolError> {
    match record.metric.as_str() {
        METRIC_HOT_RECALL => {
            if record.source.is_some() {
                return Err(ToolError::new("hot_recall 不带 source"));
            }
        }
        METRIC_RESULT => match record.source.as_deref() {
            Some("local" | "everything" | "windows_search") => {}
            Some(other) => {
                return Err(ToolError::new(format!(
                    "未知的结果来源 {other}。只能是 local、everything 或 windows_search"
                )));
            }
            None => {
                return Err(ToolError::new(
                    "result 必须带 source：local、everything 或 windows_search",
                ));
            }
        },
        other => {
            return Err(ToolError::new(format!(
                "未知的 metric {other}。只能是 hot_recall 或 result"
            )));
        }
    }
    Ok(())
}

fn group_key(record: &RawLatency) -> (u8, String, String) {
    let rank = match (record.metric.as_str(), record.source.as_deref()) {
        (METRIC_HOT_RECALL, None) => 0,
        (METRIC_RESULT, Some("local")) => 1,
        (METRIC_RESULT, Some("everything")) => 2,
        (METRIC_RESULT, Some("windows_search")) => 3,
        _ => 4,
    };
    (
        rank,
        record.metric.clone(),
        record.source.clone().unwrap_or_default(),
    )
}

#[derive(Default)]
struct GroupAccum {
    superseded: usize,
    warmup: usize,
    incomplete: usize,
    invalid: usize,
    valid: Vec<ValidSample>,
}

impl GroupAccum {
    fn push(&mut self, record: &RawLatency) {
        if record.superseded {
            self.superseded += 1;
            return;
        }
        if record.warmup {
            self.warmup += 1;
            return;
        }
        let Some(end_ns) = record.end_ns else {
            self.incomplete += 1;
            return;
        };
        if end_ns < record.start_ns {
            self.invalid += 1;
            return;
        }
        self.valid.push(ValidSample {
            seq: record.seq,
            start_ns: record.start_ns,
            end_ns,
            duration_ns: end_ns - record.start_ns,
        });
    }

    fn finish(mut self, metric: String, source: Option<String>) -> LatencyGroup {
        self.valid.sort_by(|left, right| {
            left.duration_ns
                .cmp(&right.duration_ns)
                .then(left.seq.cmp(&right.seq))
                .then(left.start_ns.cmp(&right.start_ns))
        });
        let valid_count = self.valid.len();
        let sufficient = valid_count >= MIN_VALID_SAMPLES;
        let p95_index = p95_index_1based(valid_count);
        let p95_ns =
            p95_index.and_then(|index| self.valid.get(index - 1).map(|sample| sample.duration_ns));
        LatencyGroup {
            metric,
            source,
            valid_count,
            superseded_count: self.superseded,
            warmup_excluded: self.warmup,
            incomplete_count: self.incomplete,
            invalid_count: self.invalid,
            sufficient,
            sample_status: if sufficient { "足够" } else { "不足" },
            p95_index,
            p95_ns,
            valid_samples: self.valid,
        }
    }
}

/// 给延迟模块用，避免和采样错误类型缠在一起。目前未使用独立错误。
#[allow(dead_code)]
#[derive(Debug)]
pub struct SampleError;

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        metric: &str,
        source: Option<&str>,
        seq: u64,
        duration: Option<u64>,
        warmup: bool,
        superseded: bool,
    ) -> RawLatency {
        RawLatency {
            metric: metric.to_string(),
            source: source.map(str::to_string),
            seq,
            start_ns: 0,
            end_ns: duration,
            warmup,
            superseded,
        }
    }

    #[test]
    fn sequence_1_to_100_has_p95_95_and_is_sufficient() {
        let records: Vec<_> = (1..=100)
            .map(|value| record("hot_recall", None, value, Some(value), false, false))
            .collect();
        let report = summarize(&records).unwrap();
        assert_eq!(report.groups.len(), 1);
        let group = &report.groups[0];
        assert_eq!(group.valid_count, 100);
        assert_eq!(group.p95_index, Some(95));
        assert_eq!(group.p95_ns, Some(95));
        assert!(group.sufficient);
        assert_eq!(group.sample_status, "足够");
        assert_eq!(group.valid_samples[94].duration_ns, 95);
    }

    #[test]
    fn fewer_than_100_valid_samples_are_marked_insufficient() {
        let records: Vec<_> = (1..=99)
            .map(|value| record("result", Some("local"), value, Some(value), false, false))
            .collect();
        let group = &summarize(&records).unwrap().groups[0];
        assert_eq!(group.valid_count, 99);
        assert!(!group.sufficient);
        assert_eq!(group.sample_status, "不足");
        assert_eq!(group.p95_ns, Some(95));
    }

    #[test]
    fn superseded_queries_are_excluded_and_counted() {
        let mut records: Vec<_> = (1..=100)
            .map(|value| record("result", Some("local"), value, Some(value), false, false))
            .collect();
        records.push(record(
            "result",
            Some("local"),
            1000,
            Some(1_000_000),
            false,
            true,
        ));
        records.push(record("result", Some("local"), 1001, None, true, true));
        let group = &summarize(&records).unwrap().groups[0];
        assert_eq!(group.valid_count, 100);
        assert_eq!(group.superseded_count, 2);
        assert_eq!(group.warmup_excluded, 0);
        assert_eq!(group.p95_ns, Some(95));
        assert!(
            group
                .valid_samples
                .iter()
                .all(|sample| sample.duration_ns <= 100)
        );
    }

    #[test]
    fn warmup_and_incomplete_records_are_not_samples() {
        let mut records: Vec<_> = (1..=10)
            .map(|value| record("hot_recall", None, value, Some(value), false, false))
            .collect();
        records.push(record("hot_recall", None, 11, Some(1), true, false));
        records.push(record("hot_recall", None, 12, None, false, false));
        records.push(RawLatency {
            metric: "hot_recall".to_string(),
            source: None,
            seq: 14,
            start_ns: 5,
            end_ns: Some(4),
            warmup: false,
            superseded: false,
        });
        let group = &summarize(&records).unwrap().groups[0];
        assert_eq!(group.valid_count, 10);
        assert_eq!(group.warmup_excluded, 1);
        assert_eq!(group.incomplete_count, 1);
        assert_eq!(group.invalid_count, 1);
        assert_eq!(group.sample_status, "不足");
    }

    #[test]
    fn file_sources_are_summarized_separately() {
        let records = vec![
            record("result", Some("everything"), 1, Some(10), false, false),
            record("result", Some("windows_search"), 1, Some(80), false, true),
            record("result", Some("windows_search"), 2, Some(40), false, false),
        ];
        let report = summarize(&records).unwrap();
        assert_eq!(report.groups.len(), 2);
        assert_eq!(report.groups[0].source.as_deref(), Some("everything"));
        assert_eq!(report.groups[0].p95_ns, Some(10));
        assert_eq!(report.groups[1].source.as_deref(), Some("windows_search"));
        assert_eq!(report.groups[1].valid_count, 1);
        assert_eq!(report.groups[1].superseded_count, 1);
        assert_eq!(report.groups[1].p95_ns, Some(40));
    }

    #[test]
    fn jsonl_keeps_raw_timestamps_and_reports_the_line_of_a_bad_record() {
        let text = "\n{\"metric\":\"hot_recall\",\"seq\":7,\"start_ns\":10,\"end_ns\":25,\"warmup\":false,\"superseded\":false}\n";
        let records = parse_latency_jsonl(text).unwrap();
        let group = &summarize(&records).unwrap().groups[0];
        assert_eq!(group.valid_samples[0].start_ns, 10);
        assert_eq!(group.valid_samples[0].end_ns, 25);
        assert_eq!(group.p95_ns, Some(15));
        let err = parse_latency_jsonl(
            "{\"metric\":\"hot_recall\",\"seq\":1,\"start_ns\":0,\"end_ns\":1,\"warmup\":false,\"superseded\":false}\n{",
        )
        .unwrap_err();
        assert!(err.to_string().contains("第 2 行"));
    }
}
