use std::path::Path;
use std::time::Instant;

#[derive(Debug, Clone, Copy)]
pub(super) struct AuditProgress {
    started: Instant,
    file_count: usize,
    chunk_count: usize,
}

impl AuditProgress {
    pub(super) fn new(started: Instant, file_count: usize, chunk_count: usize) -> Self {
        Self {
            started,
            file_count,
            chunk_count,
        }
    }

    pub(super) fn prepared(&self) {
        self.line(
            "prepared",
            1,
            1,
            format_args!("{} files · {} chunks", self.file_count, self.chunk_count),
        );
    }

    pub(super) fn review_started(&self, parallelism: Option<usize>) {
        let detail = match parallelism {
            Some(parallelism) => format!(
                "reviewing {} chunks · parallelism {parallelism}",
                self.chunk_count
            ),
            None => "reviewing full repo".to_string(),
        };
        self.line("review", 0, self.chunk_count, detail);
    }

    pub(super) fn review_finished(&self, completed: usize) {
        if completed < self.chunk_count && !completed.is_multiple_of(self.review_update_stride()) {
            return;
        }
        let detail = if completed >= self.chunk_count {
            "review complete".to_string()
        } else {
            format!("{completed}/{} chunks complete", self.chunk_count)
        };
        self.line("review", completed, self.chunk_count, detail);
    }

    fn review_update_stride(&self) -> usize {
        self.chunk_count.div_ceil(10).max(1)
    }

    pub(super) fn summarise_started(&self) {
        self.line("summarise", 0, 1, "deduping and ranking findings");
    }

    pub(super) fn summarise_finished(&self) {
        self.line("summarise", 1, 1, "summary complete");
    }

    pub(super) fn write_started(&self, output_path: &Path) {
        self.line("write", 0, 1, output_path.display());
    }

    pub(super) fn write_finished(&self, output_path: &Path) {
        self.line("write", 1, 1, output_path.display());
    }

    fn line(&self, label: &str, current: usize, total: usize, detail: impl std::fmt::Display) {
        crate::ui::progress(label, current, total, detail, self.started.elapsed());
    }
}
