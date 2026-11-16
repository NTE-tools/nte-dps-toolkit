use std::sync::Mutex;

use nte_dps_tool::core::diagnostics::DiagnosticRun;

#[derive(Debug, Default)]
struct DiagnosticsState {
    report: Option<DiagnosticRun>,
    revision: u64,
}

/// Ephemeral diagnostic output and its publication revision.
#[derive(Debug, Default)]
pub(crate) struct DiagnosticsRuntime {
    state: Mutex<DiagnosticsState>,
}

impl DiagnosticsRuntime {
    pub(crate) fn revision(&self) -> u64 {
        self.state.lock().expect("diagnostics state lock").revision
    }

    pub(crate) fn snapshot(&self) -> (u64, Option<DiagnosticRun>) {
        let state = self.state.lock().expect("diagnostics state lock");
        (state.revision, state.report.clone())
    }

    /// Stores one complete diagnostic run. Equal output is a semantic no-op.
    pub(crate) fn store(&self, report: DiagnosticRun) -> bool {
        let mut state = self.state.lock().expect("diagnostics state lock");
        if state.report.as_ref() == Some(&report) {
            return false;
        }
        state.report = Some(report);
        state.revision = next_revision(state.revision);
        true
    }
}

const fn next_revision(revision: u64) -> u64 {
    revision.wrapping_add(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_report_is_a_noop_and_changed_report_bumps_once() {
        let runtime = DiagnosticsRuntime::default();
        let report = DiagnosticRun::default();
        assert!(runtime.store(report.clone()));
        assert!(!runtime.store(report));
        assert_eq!(runtime.revision(), 1);

        let mut changed = DiagnosticRun::default();
        changed.environment.manual_device = true;
        assert!(runtime.store(changed.clone()));
        assert_eq!(runtime.snapshot(), (2, Some(changed)));
    }
}
