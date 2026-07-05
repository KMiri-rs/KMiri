#[derive(
    PartialEq, PartialOrd, Eq, Ord, Clone, Debug, Default, serde::Deserialize, serde::Serialize,
)]
pub struct FunctionInstanceInfo {
    pub instance: String,
    pub source_file: String,
    pub line_start: u16,
    pub line_end: u16,
}
