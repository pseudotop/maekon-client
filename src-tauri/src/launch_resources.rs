use anyhow::Result;
use std::path::Path;
use tauri::AppHandle;
use tokio::runtime::Handle;

use crate::background_runtime::BackgroundRuntimeCoordinator;
use crate::storage_runtime::{StorageRuntimeBuilder, StorageRuntimeBundle};
use crate::update_runtime::{UpdateRuntimeBuilder, UpdateRuntimeBundle};

pub(crate) struct LaunchCoreResources<'a> {
    pub(crate) update_runtime: UpdateRuntimeBundle,
    pub(crate) storage_runtime: StorageRuntimeBundle,
    pub(crate) background_runtime: BackgroundRuntimeCoordinator<'a>,
}

pub(crate) struct LaunchCoreResourcesBuilder<'a> {
    config: &'a oneshim_core::config::AppConfig,
    db_path: &'a Path,
    data_dir_path: &'a Path,
    runtime_handle: &'a Handle,
    app_handle: AppHandle,
}

impl<'a> LaunchCoreResourcesBuilder<'a> {
    pub(crate) fn new(
        config: &'a oneshim_core::config::AppConfig,
        db_path: &'a Path,
        data_dir_path: &'a Path,
        runtime_handle: &'a Handle,
        app_handle: AppHandle,
    ) -> Self {
        Self {
            config,
            db_path,
            data_dir_path,
            runtime_handle,
            app_handle,
        }
    }

    pub(crate) fn build(&self) -> Result<LaunchCoreResources<'a>> {
        let update_runtime =
            UpdateRuntimeBuilder::new(&self.config.update, self.runtime_handle).build_and_spawn();
        let storage_runtime = StorageRuntimeBuilder::new(
            self.db_path,
            self.data_dir_path,
            self.config.storage.retention_days,
        )
        .build()?;
        let background_runtime =
            BackgroundRuntimeCoordinator::new(self.runtime_handle, self.app_handle.clone());

        Ok(LaunchCoreResources {
            update_runtime,
            storage_runtime,
            background_runtime,
        })
    }
}
