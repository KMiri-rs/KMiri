#[derive(
    PartialEq, PartialOrd, Eq, Ord, Clone, Debug, Default, serde::Deserialize, serde::Serialize,
)]
pub struct FunctionInstanceInfo {
    pub instance: String,
    pub source_file: String,
    pub line_start: u16,
    pub line_end: u16,
}

impl FunctionInstanceInfo {
    fn dedup_fn(&self, other: &Self) -> bool {
        self.instance == other.instance
            && self.source_file == other.source_file
            && other.line_start <= self.line_start
            && self.line_end <= other.line_end
    }
}

pub fn dedup(v_fn: &mut Vec<FunctionInstanceInfo>) {
    let old = v_fn.len();
    v_fn.sort_unstable_by(|a, b| b.cmp(a));
    v_fn.dedup_by(|a, b| a.dedup_fn(b));
    println!("len: old={old} new={}", v_fn.len());
}

#[test]
fn dedup_fn() {
    let mut v_fn = vec![FunctionInstanceInfo::default(); 3];
    v_fn[0].line_start = 1;
    v_fn[0].line_end = 1;

    v_fn[1].line_start = 1;
    v_fn[1].line_end = 3;

    dedup(&mut v_fn);

    assert_eq!(
        v_fn,
        [
            FunctionInstanceInfo {
                line_start: 1,
                line_end: 3,
                ..Default::default()
            },
            FunctionInstanceInfo {
                line_start: 0,
                line_end: 0,
                ..Default::default()
            },
        ]
    );
}

pub const ENV_INNER_DIR_TARGET: &str = "__KMIRI_DIR_TARGET";
pub const ANALYSIS: &str = "analysis";
pub const ANALYSIS_LOG: &str = "analysis.log";
pub const ANALYSIS_JSON: &str = "analysis.json";
