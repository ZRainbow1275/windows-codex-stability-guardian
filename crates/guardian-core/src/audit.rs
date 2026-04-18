use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct AuditRecord {
    pub action: String,
    pub outcome: String,
    pub backup_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupRecord {
    pub source: String,
    pub destination: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepairAuditRecord {
    pub timestamp: String,
    pub action: String,
    pub outcome: String,
    pub state_db_path: String,
    pub stale_rows_before: i64,
    pub stale_rows_after: i64,
    pub active_version: Option<String>,
    pub backup_path: Option<String>,
    pub stdout_excerpt: Vec<String>,
    pub stderr_excerpt: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DockerRepairAuditRecord {
    pub timestamp: String,
    pub action: String,
    pub outcome: String,
    pub wslconfig_path: String,
    pub backup_path: Option<String>,
    pub created_file: bool,
    pub restart_required: bool,
    pub runtime_anomaly_detected: bool,
    pub runtime_anomaly_after: bool,
    pub runtime_repair_attempted: bool,
    pub runtime_repair_blocked: bool,
    pub runtime_action: Option<String>,
    pub runtime_block_reason: Option<String>,
    pub runtime_error: Option<String>,
    pub runtime_wait_poll_count: u32,
    pub runtime_wait_stable_success_polls: u32,
    pub runtime_wait_timed_out: bool,
    pub runtime_wait_elapsed_ms: u64,
    pub running_containers_before: Option<u64>,
    pub docker_desktop_status_before: Option<String>,
    pub docker_desktop_status_after: Option<String>,
    pub missing_keys_before: Vec<String>,
    pub missing_keys_after: Vec<String>,
    pub baseline_memory: String,
    pub baseline_processors: u64,
    pub baseline_swap: String,
    pub baseline_auto_memory_reclaim: String,
}
