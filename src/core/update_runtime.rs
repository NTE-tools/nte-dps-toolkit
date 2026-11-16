use std::sync::{Mutex, MutexGuard};

use crate::{
    core::update::{AvailableComponentUpdate, UpdateComponent},
    storage::update::PreparedUpdate,
};

pub const UPDATE_RUNTIME_UNAVAILABLE_MESSAGE_KEY: &str = "Update operation did not finish.";

#[derive(Clone, Debug)]
pub struct UpdateRuntimeSnapshot {
    pub status: &'static str,
    pub message_key: &'static str,
    pub message_arguments: Vec<String>,
    pub available: Vec<AvailableComponentUpdate>,
    pub active_component: Option<UpdateComponent>,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub prepared: Option<PreparedUpdate>,
}

impl Default for UpdateRuntimeSnapshot {
    fn default() -> Self {
        Self {
            status: "idle",
            message_key: "Updates have not been checked in this session",
            message_arguments: Vec::new(),
            available: Vec::new(),
            active_component: None,
            downloaded_bytes: 0,
            total_bytes: 0,
            prepared: None,
        }
    }
}

impl UpdateRuntimeSnapshot {
    pub fn unavailable() -> Self {
        Self {
            status: "unavailable",
            message_key: UPDATE_RUNTIME_UNAVAILABLE_MESSAGE_KEY,
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateActionError {
    Busy,
    Unavailable,
    NotPrepared,
    RuntimeUnavailable,
}

#[derive(Default)]
pub struct UpdateRuntimeService {
    runtime: Mutex<UpdateRuntimeSnapshot>,
}

impl UpdateRuntimeService {
    pub fn snapshot(&self) -> Result<UpdateRuntimeSnapshot, UpdateActionError> {
        Ok(self.lock()?.clone())
    }

    pub fn begin_check(&self) -> Result<(), UpdateActionError> {
        let mut update = self.lock()?;
        if update_is_busy(update.status) || update.prepared.is_some() {
            return Err(UpdateActionError::Busy);
        }
        update.status = "checking";
        update.message_key = "Checking for updates...";
        update.message_arguments.clear();
        update.active_component = None;
        update.downloaded_bytes = 0;
        update.total_bytes = 0;
        Ok(())
    }

    pub fn finish_check(
        &self,
        available: Vec<AvailableComponentUpdate>,
    ) -> Result<(), UpdateActionError> {
        let mut update = self.lock()?;
        update.prepared = None;
        update.active_component = None;
        update.downloaded_bytes = 0;
        update.total_bytes = 0;
        if available.is_empty() {
            update.available.clear();
            update.status = "up-to-date";
            update.message_key = "All available update components are up to date";
            update.message_arguments.clear();
        } else {
            let preferred = available
                .iter()
                .find(|item| item.component == UpdateComponent::App)
                .unwrap_or(&available[0]);
            update.status = "available";
            update.message_key = match preferred.component {
                UpdateComponent::App => "Version {} is available",
            };
            update.message_arguments = vec![preferred.version.to_string()];
            update.available = available;
        }
        Ok(())
    }

    pub fn fail_check(&self, message_key: &'static str) -> Result<(), UpdateActionError> {
        let mut update = self.lock()?;
        update.status =
            if message_key == "The official update channel is not configured in this build" {
                "not-configured"
            } else {
                "error"
            };
        update.message_key = message_key;
        update.message_arguments.clear();
        update.available.clear();
        update.prepared = None;
        update.active_component = None;
        update.downloaded_bytes = 0;
        update.total_bytes = 0;
        Ok(())
    }

    pub fn begin_download(
        &self,
        component: UpdateComponent,
    ) -> Result<AvailableComponentUpdate, UpdateActionError> {
        let mut update = self.lock()?;
        if update_is_busy(update.status) || update.prepared.is_some() {
            return Err(UpdateActionError::Busy);
        }
        let selected = update
            .available
            .iter()
            .find(|item| item.component == component)
            .cloned()
            .ok_or(UpdateActionError::Unavailable)?;
        update.status = "downloading";
        update.message_key = match component {
            UpdateComponent::App => "Downloading verified update...",
        };
        update.message_arguments.clear();
        update.active_component = Some(component);
        update.downloaded_bytes = 0;
        update.total_bytes = selected.artifact_size;
        Ok(selected)
    }

    pub fn update_download_progress(
        &self,
        component: UpdateComponent,
        downloaded_bytes: u64,
        total_bytes: u64,
    ) -> Result<bool, UpdateActionError> {
        let mut update = self.lock()?;
        if update.status != "downloading" || update.active_component != Some(component) {
            return Ok(false);
        }
        let downloaded_bytes = downloaded_bytes.min(total_bytes);
        if update.downloaded_bytes == downloaded_bytes && update.total_bytes == total_bytes {
            return Ok(false);
        }
        update.downloaded_bytes = downloaded_bytes;
        update.total_bytes = total_bytes;
        Ok(true)
    }

    pub fn finish_download(&self, prepared: PreparedUpdate) -> Result<(), UpdateActionError> {
        let component = prepared.component();
        let version = prepared.version().to_string();
        let mut update = self.lock()?;
        update.status = "ready";
        update.message_key = match component {
            UpdateComponent::App => "Version {} is ready to install",
        };
        update.message_arguments = vec![version];
        update.active_component = None;
        update.downloaded_bytes = update.total_bytes;
        update.prepared = Some(prepared);
        Ok(())
    }

    pub fn begin_install(&self) -> Result<PreparedUpdate, UpdateActionError> {
        let mut update = self.lock()?;
        if update_is_busy(update.status) {
            return Err(UpdateActionError::Busy);
        }
        let prepared = update
            .prepared
            .as_ref()
            .cloned()
            .ok_or(UpdateActionError::NotPrepared)?;
        update.status = match prepared.component() {
            UpdateComponent::App => "restarting",
        };
        update.message_key = match prepared.component() {
            UpdateComponent::App => "Restarting to install the update...",
        };
        update.message_arguments.clear();
        update.active_component = Some(prepared.component());
        Ok(prepared)
    }

    pub fn fail_operation(&self, message_key: &'static str) -> Result<(), UpdateActionError> {
        let mut update = self.lock()?;
        update.status = "error";
        update.message_key = message_key;
        update.message_arguments.clear();
        update.active_component = None;
        update.downloaded_bytes = 0;
        update.total_bytes = 0;
        Ok(())
    }

    pub fn prepared_component(&self) -> Result<Option<UpdateComponent>, UpdateActionError> {
        Ok(self
            .lock()?
            .prepared
            .as_ref()
            .map(PreparedUpdate::component))
    }

    fn lock(&self) -> Result<MutexGuard<'_, UpdateRuntimeSnapshot>, UpdateActionError> {
        self.runtime
            .lock()
            .map_err(|_| UpdateActionError::RuntimeUnavailable)
    }
}

fn update_is_busy(status: &str) -> bool {
    matches!(
        status,
        "checking" | "downloading" | "installing" | "restarting"
    )
}
