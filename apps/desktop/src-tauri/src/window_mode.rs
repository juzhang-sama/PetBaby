use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use serde::Serialize;
use tauri::{Emitter, Manager};

use crate::{
    platform::{DesktopAttachOutcome, PlatformAdapter, WindowHostSnapshot},
    preferences,
    windowing::{
        reduce_window_mode, DesktopHostAttempt, DesktopHostState, SuppressionReason, WindowMode,
        WindowModeAction, WindowModeEvent, WindowModeState,
    },
};

const MAX_REQUEST_ID_BYTES: usize = 128;
const MAX_COMPLETED_TRANSITIONS: usize = 128;
const MAX_CANONICAL_REVISION: u64 = 9_007_199_254_740_991;
const STARTUP_RESTORE_CANCELLED: &str =
    "startup window mode restore was cancelled by an explicit request";
const DESKTOP_HOST_HEALTH_INTERVAL: Duration = Duration::from_secs(2);
const DESKTOP_RECOVERY_BACKOFFS: [Duration; 4] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
];
pub const RUNTIME_PAUSE_EVENT: &str = "pet-runtime:pause";
pub const RUNTIME_RESUME_EVENT: &str = "pet-runtime:resume";
pub const SNAPSHOT_CHANGED_EVENT: &str = "window-mode:changed";

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DesktopStrategy {
    WorkerW,
    BottomFallback,
}

impl From<DesktopAttachOutcome> for DesktopStrategy {
    fn from(value: DesktopAttachOutcome) -> Self {
        match value {
            DesktopAttachOutcome::WorkerW { .. } => Self::WorkerW,
            DesktopAttachOutcome::BottomFallback => Self::BottomFallback,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WindowModeSnapshot {
    pub revision: u64,
    pub desired_mode: WindowMode,
    pub actual_mode: Option<WindowMode>,
    pub desktop_strategy: Option<DesktopStrategy>,
    pub user_visible: bool,
    pub suppressions: Vec<SuppressionReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeAckPhase {
    Paused,
    Resumed,
}

impl RuntimeAckPhase {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "paused" => Ok(Self::Paused),
            "resumed" => Ok(Self::Resumed),
            _ => Err(format!(
                "unsupported window mode runtime ACK phase: {value}"
            )),
        }
    }

    fn event(self) -> &'static str {
        match self {
            Self::Paused => RUNTIME_PAUSE_EVENT,
            Self::Resumed => RUNTIME_RESUME_EVENT,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Paused => "paused",
            Self::Resumed => "resumed",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeTransitionPayload<'a> {
    request_id: &'a str,
    cycle: u64,
    phase: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    effective_visible: Option<bool>,
}

pub trait WindowModeIo: Send + Sync {
    fn emit_runtime(
        &self,
        request_id: &str,
        cycle: u64,
        phase: RuntimeAckPhase,
        effective_visible: Option<bool>,
    ) -> Result<(), String>;
    fn capture_window_host(&self) -> Result<WindowHostSnapshot, String>;
    fn attach_desktop_host(
        &self,
        snapshot: &WindowHostSnapshot,
    ) -> Result<DesktopAttachOutcome, String>;
    fn restore_window_host(&self, snapshot: &WindowHostSnapshot) -> Result<(), String>;
    fn set_visible(&self, visible: bool) -> Result<(), String>;
    fn persist(&self, mode: WindowMode, user_visible: bool) -> Result<(), String>;
    fn desktop_host_alive(&self, host: DesktopAttachOutcome) -> Result<bool, String>;
    fn publish_snapshot(&self, snapshot: &WindowModeSnapshot) -> Result<(), String>;
    fn report_recovery(&self, message: &str);
}

pub struct TauriWindowModeIo {
    app: tauri::AppHandle,
    platform: Arc<dyn PlatformAdapter>,
    hwnd: isize,
    preferences_path: PathBuf,
    preferences_lock: Arc<Mutex<()>>,
}

impl TauriWindowModeIo {
    pub fn new(
        app: tauri::AppHandle,
        platform: Arc<dyn PlatformAdapter>,
        hwnd: isize,
        preferences_path: PathBuf,
        preferences_lock: Arc<Mutex<()>>,
    ) -> Self {
        Self {
            app,
            platform,
            hwnd,
            preferences_path,
            preferences_lock,
        }
    }
}

impl WindowModeIo for TauriWindowModeIo {
    fn emit_runtime(
        &self,
        request_id: &str,
        cycle: u64,
        phase: RuntimeAckPhase,
        effective_visible: Option<bool>,
    ) -> Result<(), String> {
        self.app
            .emit_to(
                "pet",
                phase.event(),
                RuntimeTransitionPayload {
                    request_id,
                    cycle,
                    phase: phase.as_str(),
                    effective_visible,
                },
            )
            .map_err(|error| error.to_string())
    }

    fn capture_window_host(&self) -> Result<WindowHostSnapshot, String> {
        self.platform
            .capture_window_host(self.hwnd)
            .map_err(|error| error.to_string())
    }

    fn attach_desktop_host(
        &self,
        snapshot: &WindowHostSnapshot,
    ) -> Result<DesktopAttachOutcome, String> {
        self.platform
            .attach_desktop_host(self.hwnd, snapshot)
            .map_err(|error| error.to_string())
    }

    fn restore_window_host(&self, snapshot: &WindowHostSnapshot) -> Result<(), String> {
        self.platform
            .restore_window_host(self.hwnd, snapshot)
            .map_err(|error| error.to_string())
    }

    fn set_visible(&self, visible: bool) -> Result<(), String> {
        let window = self
            .app
            .get_webview_window("pet")
            .ok_or_else(|| "pet window missing".to_owned())?;
        if visible {
            window.show()
        } else {
            window.hide()
        }
        .map_err(|error| error.to_string())
    }

    fn persist(&self, mode: WindowMode, user_visible: bool) -> Result<(), String> {
        let _lease = self
            .preferences_lock
            .lock()
            .map_err(|_| "preferences lock poisoned".to_owned())?;
        preferences::update_window_mode(&self.preferences_path, mode, user_visible)
            .map_err(|error| error.to_string())
    }

    fn desktop_host_alive(&self, host: DesktopAttachOutcome) -> Result<bool, String> {
        self.platform
            .desktop_host_alive(self.hwnd, host)
            .map_err(|error| error.to_string())
    }

    fn publish_snapshot(&self, snapshot: &WindowModeSnapshot) -> Result<(), String> {
        crate::publish_window_mode_snapshot(&self.app, snapshot)
    }

    fn report_recovery(&self, message: &str) {
        eprintln!("[desktop-pet] desktop host recovery: {message}");
    }
}

trait RecoveryWait: Send + Sync {
    fn wait(&self, delay: Duration, cancellation: &RecoveryCancellation) -> bool;
}

struct RecoveryCancellation {
    cancelled: Mutex<bool>,
    changed: Condvar,
}

impl RecoveryCancellation {
    fn new() -> Self {
        Self {
            cancelled: Mutex::new(false),
            changed: Condvar::new(),
        }
    }

    fn cancel(&self) {
        if let Ok(mut cancelled) = self.cancelled.lock() {
            *cancelled = true;
            self.changed.notify_all();
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.lock().map(|value| *value).unwrap_or(true)
    }
}

struct SystemRecoveryWait;

impl RecoveryWait for SystemRecoveryWait {
    fn wait(&self, delay: Duration, cancellation: &RecoveryCancellation) -> bool {
        let Ok(cancelled) = cancellation.cancelled.lock() else {
            return false;
        };
        if *cancelled {
            return false;
        }
        let Ok((cancelled, _)) = cancellation.changed.wait_timeout(cancelled, delay) else {
            return false;
        };
        !*cancelled
    }
}

#[derive(Debug, Clone)]
struct ActiveTransition {
    request_id: String,
    requested_mode: WindowMode,
    next_cycle: u64,
    pre_commit: bool,
    startup_lease: Option<StartupRestoreLease>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartupRestoreLease(u64);

#[derive(Debug, Clone)]
struct RuntimeAckLease {
    request_id: String,
    cycle: u64,
    phase: RuntimeAckPhase,
    received: bool,
}

#[derive(Debug, Clone)]
struct CompletedTransition {
    requested_mode: WindowMode,
    result: Result<WindowModeSnapshot, String>,
}

#[derive(Debug, Clone)]
struct RecoveryFailureGate {
    generation: u64,
    error: String,
}

struct ControllerData {
    canonical_revision: u64,
    canonical_payload: CanonicalPayload,
    startup_generation: u64,
    startup_active: Option<StartupRestoreLease>,
    machine: WindowModeState,
    desired_mode: WindowMode,
    actual_mode: Option<WindowMode>,
    degraded: bool,
    visibility_degraded: bool,
    runtime_synchronized: bool,
    desktop_strategy: Option<DesktopStrategy>,
    companion_snapshot: Option<WindowHostSnapshot>,
    active: Option<ActiveTransition>,
    completed: BTreeMap<String, CompletedTransition>,
    completed_order: VecDeque<String>,
    side_operation_in_progress: bool,
    pending_precommit_fullscreen: Option<bool>,
    runtime_ack: Option<RuntimeAckLease>,
    visibility_sequence: u64,
    runtime_ready_epoch: u64,
    recovery: Option<Arc<RecoveryCancellation>>,
    recovery_generation: u64,
    active_recovery_generation: Option<u64>,
    recovery_waiters: usize,
    recovery_failure_gate: Option<RecoveryFailureGate>,
    shutting_down: bool,
    shutdown_complete: bool,
}

impl ControllerData {
    fn startup_is_current(&self, lease: StartupRestoreLease) -> bool {
        self.startup_generation == lease.0 && self.startup_active == Some(lease)
    }

    fn payload(&self) -> CanonicalPayload {
        CanonicalPayload {
            desired_mode: self.desired_mode,
            actual_mode: self.actual_mode,
            desktop_strategy: self.desktop_strategy,
            user_visible: self.machine.user_visible(),
            suppressions: {
                let mut suppressions: Vec<_> =
                    self.machine.suppressions().iter().copied().collect();
                if (self.degraded || self.visibility_degraded)
                    && !suppressions.contains(&SuppressionReason::Transition)
                {
                    suppressions.push(SuppressionReason::Transition);
                }
                suppressions
            },
        }
    }

    fn stamp_canonical(&mut self) {
        let payload = self.payload();
        if payload != self.canonical_payload {
            self.canonical_revision = self
                .canonical_revision
                .saturating_add(1)
                .min(MAX_CANONICAL_REVISION);
            self.canonical_payload = payload;
        }
    }

    fn snapshot(&mut self) -> WindowModeSnapshot {
        self.stamp_canonical();
        WindowModeSnapshot {
            revision: self.canonical_revision,
            desired_mode: self.canonical_payload.desired_mode,
            actual_mode: self.canonical_payload.actual_mode,
            desktop_strategy: self.canonical_payload.desktop_strategy,
            user_visible: self.canonical_payload.user_visible,
            suppressions: self.canonical_payload.suppressions.clone(),
        }
    }

    fn cache_completion(
        &mut self,
        request_id: String,
        requested_mode: WindowMode,
        result: Result<WindowModeSnapshot, String>,
    ) {
        if !self.completed.contains_key(&request_id) {
            self.completed_order.push_back(request_id.clone());
        }
        self.completed.insert(
            request_id,
            CompletedTransition {
                requested_mode,
                result,
            },
        );
        while self.completed_order.len() > MAX_COMPLETED_TRANSITIONS {
            if let Some(expired) = self.completed_order.pop_front() {
                self.completed.remove(&expired);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CanonicalPayload {
    desired_mode: WindowMode,
    actual_mode: Option<WindowMode>,
    desktop_strategy: Option<DesktopStrategy>,
    user_visible: bool,
    suppressions: Vec<SuppressionReason>,
}

pub struct WindowModeController {
    io: Arc<dyn WindowModeIo>,
    ack_timeout: Duration,
    data: Mutex<ControllerData>,
    changed: Condvar,
    recovery_wait: Arc<dyn RecoveryWait>,
    health_monitor_cancel: Mutex<Option<Arc<RecoveryCancellation>>>,
}

pub type SharedWindowModeController = Arc<WindowModeController>;

impl WindowModeController {
    pub fn new(io: Arc<dyn WindowModeIo>, user_visible: bool) -> Self {
        Self::with_timeout_and_recovery_wait(
            io,
            user_visible,
            Duration::from_secs(2),
            Arc::new(SystemRecoveryWait),
        )
    }

    #[cfg(test)]
    fn with_timeout(io: Arc<dyn WindowModeIo>, user_visible: bool, ack_timeout: Duration) -> Self {
        Self::with_timeout_and_recovery_wait(
            io,
            user_visible,
            ack_timeout,
            Arc::new(SystemRecoveryWait),
        )
    }

    fn with_timeout_and_recovery_wait(
        io: Arc<dyn WindowModeIo>,
        user_visible: bool,
        ack_timeout: Duration,
        recovery_wait: Arc<dyn RecoveryWait>,
    ) -> Self {
        let mut machine = WindowModeState::new();
        if !user_visible {
            machine =
                reduce_window_mode(machine, WindowModeEvent::UserVisibilityChanged(false)).state;
        }
        let canonical_payload = CanonicalPayload {
            desired_mode: WindowMode::Companion,
            actual_mode: Some(WindowMode::Companion),
            desktop_strategy: None,
            user_visible,
            suppressions: Vec::new(),
        };
        Self {
            io,
            ack_timeout,
            data: Mutex::new(ControllerData {
                canonical_revision: 0,
                canonical_payload,
                startup_generation: 0,
                startup_active: None,
                machine,
                desired_mode: WindowMode::Companion,
                actual_mode: Some(WindowMode::Companion),
                degraded: false,
                visibility_degraded: false,
                runtime_synchronized: true,
                desktop_strategy: None,
                companion_snapshot: None,
                active: None,
                completed: BTreeMap::new(),
                completed_order: VecDeque::new(),
                side_operation_in_progress: false,
                pending_precommit_fullscreen: None,
                runtime_ack: None,
                visibility_sequence: 0,
                runtime_ready_epoch: 0,
                recovery: None,
                recovery_generation: 0,
                active_recovery_generation: None,
                recovery_waiters: 0,
                recovery_failure_gate: None,
                shutting_down: false,
                shutdown_complete: false,
            }),
            changed: Condvar::new(),
            recovery_wait,
            health_monitor_cancel: Mutex::new(None),
        }
    }

    pub fn snapshot(&self) -> Result<WindowModeSnapshot, String> {
        Ok(self.lock_data()?.snapshot())
    }

    pub fn start_health_monitor(self: &Arc<Self>) -> Result<(), String> {
        let cancellation = Arc::new(RecoveryCancellation::new());
        {
            let mut current = self
                .health_monitor_cancel
                .lock()
                .map_err(|_| "desktop host monitor lock poisoned".to_owned())?;
            if current.is_some() {
                return Ok(());
            }
            *current = Some(cancellation.clone());
        }
        let controller = Arc::downgrade(self);
        std::thread::spawn(move || {
            while SystemRecoveryWait.wait(DESKTOP_HOST_HEALTH_INTERVAL, &cancellation) {
                let Some(controller) = controller.upgrade() else {
                    break;
                };
                if let Err(error) = controller.check_desktop_host() {
                    controller
                        .io
                        .report_recovery(&format!("health check failed: {error}"));
                }
            }
        });
        Ok(())
    }

    pub fn check_desktop_host(&self) -> Result<WindowModeSnapshot, String> {
        let result = self.check_desktop_host_inner();
        self.publish_terminal(result)
    }

    fn check_desktop_host_inner(&self) -> Result<WindowModeSnapshot, String> {
        let (host, cancellation, request_id) = {
            let mut data = self.lock_data()?;
            if data.shutting_down
                || data.actual_mode != Some(WindowMode::Desktop)
                || data.active.is_some()
                || data.side_operation_in_progress
                || data.recovery.is_some()
            {
                return Ok(data.snapshot());
            }
            let host = desktop_outcome(&data.machine, data.desktop_strategy)
                .ok_or_else(|| "actual desktop mode has no live host strategy".to_owned())?;
            let cancellation = Arc::new(RecoveryCancellation::new());
            let request_id = crate::creation::domain::new_entity_id("explorer-recovery");
            validate_request_id(&request_id)?;
            data.recovery_generation = data
                .recovery_generation
                .checked_add(1)
                .ok_or_else(|| "desktop recovery generation overflow".to_owned())?;
            let generation = data.recovery_generation;
            data.active_recovery_generation = Some(generation);
            data.recovery = Some(cancellation.clone());
            data.side_operation_in_progress = true;
            (host, cancellation, request_id)
        };

        match self.io.desktop_host_alive(host) {
            Ok(true) => {
                self.finish_recovery_lease(&request_id)?;
                self.snapshot()
            }
            Err(error) => {
                self.finish_recovery_lease(&request_id)?;
                Err(error)
            }
            Ok(false) => self.recover_desktop_host(request_id, cancellation),
        }
    }

    pub fn shutdown(&self) {
        if let Ok(mut monitor) = self.health_monitor_cancel.lock() {
            if let Some(cancellation) = monitor.take() {
                cancellation.cancel();
            }
        }
        let snapshot = {
            let Ok(mut data) = self.lock_data() else {
                self.io
                    .report_recovery("shutdown could not lock the controller");
                return;
            };
            if data.shutdown_complete {
                return;
            }
            data.shutting_down = true;
            if let Some(cancellation) = &data.recovery {
                cancellation.cancel();
            }
            while data.active.is_some() || data.side_operation_in_progress {
                let Ok(next) = self.changed.wait(data) else {
                    self.io.report_recovery("shutdown wait lock was poisoned");
                    return;
                };
                data = next;
            }
            data.shutdown_complete = true;
            if matches!(data.actual_mode, Some(WindowMode::Desktop) | None) {
                data.companion_snapshot
            } else {
                None
            }
        };
        if let Some(snapshot) = snapshot {
            if let Err(error) = self.io.restore_window_host(&snapshot) {
                self.io
                    .report_recovery(&format!("shutdown restore failed: {error}"));
            }
        }
    }

    pub fn set_mode(
        &self,
        request_id: String,
        requested_mode: WindowMode,
    ) -> Result<WindowModeSnapshot, String> {
        let result = self.set_mode_inner(request_id, requested_mode);
        self.publish_terminal(result)
    }

    fn set_mode_inner(
        &self,
        request_id: String,
        requested_mode: WindowMode,
    ) -> Result<WindowModeSnapshot, String> {
        self.set_mode_inner_with_startup(request_id, requested_mode, None)
    }

    fn set_mode_inner_with_startup(
        &self,
        request_id: String,
        requested_mode: WindowMode,
        startup_lease: Option<StartupRestoreLease>,
    ) -> Result<WindowModeSnapshot, String> {
        validate_request_id(&request_id)?;
        let (old, runtime_recovery_only) = {
            let mut data = self.lock_data()?;
            let mut waited_recovery_generation = None;
            loop {
                if data.shutting_down {
                    return Err("window mode controller is shutting down".to_owned());
                }
                if let Some(recovery) = data.recovery.clone() {
                    if waited_recovery_generation.is_none() {
                        waited_recovery_generation = data.active_recovery_generation;
                        data.recovery_waiters = data.recovery_waiters.saturating_add(1);
                    }
                    recovery.cancel();
                    data = self
                        .changed
                        .wait(data)
                        .map_err(|_| "window mode controller lock poisoned".to_owned())?;
                    continue;
                }
                let waited_generation = waited_recovery_generation.take();
                if waited_generation.is_some() {
                    data.recovery_waiters = data.recovery_waiters.saturating_sub(1);
                }
                if let Some(gate) = &data.recovery_failure_gate {
                    if waited_generation.is_none()
                        || waited_generation.is_some_and(|generation| generation <= gate.generation)
                    {
                        return Err(gate.error.clone());
                    }
                }
                if let Some(completed) = data.completed.get(&request_id) {
                    if completed.requested_mode != requested_mode {
                        return Err(format!(
                            "window mode requestId is already bound to {:?}",
                            completed.requested_mode
                        ));
                    }
                    return completed.result.clone();
                }
                if let Some(active) = &data.active {
                    if active.request_id != request_id {
                        return Err(format!(
                            "window mode transition {} is already in progress",
                            active.request_id
                        ));
                    }
                    if active.requested_mode != requested_mode {
                        return Err(format!(
                            "window mode requestId is already bound to {:?}",
                            active.requested_mode
                        ));
                    }
                    data = self
                        .changed
                        .wait(data)
                        .map_err(|_| "window mode controller lock poisoned".to_owned())?;
                    continue;
                }
                if data.side_operation_in_progress {
                    return Err("window visibility operation is in progress".to_owned());
                }
                if Some(requested_mode) == data.actual_mode
                    && data.desired_mode == requested_mode
                    && data.machine.transition().is_none()
                    && !data.degraded
                {
                    if data.runtime_synchronized && !data.visibility_degraded {
                        let snapshot = data.snapshot();
                        data.cache_completion(request_id, requested_mode, Ok(snapshot.clone()));
                        return Ok(snapshot);
                    }
                    let old = TransitionBaseline {
                        machine: data.machine.clone(),
                        desired_mode: data.desired_mode,
                        actual_mode: data.actual_mode,
                        degraded: data.degraded,
                        visibility_degraded: data.visibility_degraded,
                        desktop_strategy: data.desktop_strategy,
                        companion_snapshot: data.companion_snapshot,
                    };
                    if startup_lease.is_some_and(|lease| !data.startup_is_current(lease)) {
                        return Err(STARTUP_RESTORE_CANCELLED.to_owned());
                    }
                    data.active = Some(ActiveTransition {
                        request_id: request_id.clone(),
                        requested_mode,
                        next_cycle: 1,
                        pre_commit: false,
                        startup_lease,
                    });
                    break (old, true);
                }

                let old = TransitionBaseline {
                    machine: data.machine.clone(),
                    desired_mode: data.desired_mode,
                    actual_mode: data.actual_mode,
                    degraded: data.degraded,
                    visibility_degraded: data.visibility_degraded,
                    desktop_strategy: data.desktop_strategy,
                    companion_snapshot: data.companion_snapshot,
                };
                if startup_lease.is_some_and(|lease| !data.startup_is_current(lease)) {
                    return Err(STARTUP_RESTORE_CANCELLED.to_owned());
                }
                if !(data.degraded && requested_mode == WindowMode::Companion) {
                    data.machine = reduce_window_mode(
                        data.machine.clone(),
                        WindowModeEvent::RequestMode(requested_mode),
                    )
                    .state;
                }
                data.desired_mode = requested_mode;
                data.active = Some(ActiveTransition {
                    request_id: request_id.clone(),
                    requested_mode,
                    next_cycle: 1,
                    pre_commit: true,
                    startup_lease,
                });
                data.stamp_canonical();
                break (old, false);
            }
        };

        if startup_lease.is_some_and(|lease| {
            self.lock_data()
                .map(|data| !data.startup_is_current(lease))
                .unwrap_or(true)
        }) {
            let mut data = self.lock_data()?;
            if data
                .active
                .as_ref()
                .is_some_and(|active| active.request_id == request_id)
            {
                data.machine = old.machine.clone();
                data.desired_mode = old.desired_mode;
                data.actual_mode = old.actual_mode;
                data.degraded = old.degraded;
                data.visibility_degraded = old.visibility_degraded;
                data.desktop_strategy = old.desktop_strategy;
                data.companion_snapshot = old.companion_snapshot;
                data.active = None;
                data.stamp_canonical();
                self.changed.notify_all();
            }
            return Err(STARTUP_RESTORE_CANCELLED.to_owned());
        }

        let result = if runtime_recovery_only {
            self.execute_runtime_recovery(&request_id, requested_mode, &old)
        } else {
            self.execute_transition(&request_id, requested_mode, &old)
        };
        let mut data = self.lock_data()?;
        if data
            .active
            .as_ref()
            .is_some_and(|active| active.request_id == request_id)
        {
            data.active = None;
        }
        if data
            .runtime_ack
            .as_ref()
            .is_some_and(|lease| lease.request_id == request_id)
        {
            data.runtime_ack = None;
        }
        data.cache_completion(request_id, requested_mode, result.clone());
        self.changed.notify_all();
        result
    }

    pub fn runtime_ack(
        &self,
        request_id: &str,
        cycle: u64,
        phase: RuntimeAckPhase,
    ) -> Result<bool, String> {
        validate_request_id(request_id)?;
        let mut data = self.lock_data()?;
        let Some(lease) = data.runtime_ack.as_mut() else {
            return Ok(false);
        };
        if lease.request_id != request_id
            || lease.cycle != cycle
            || lease.phase != phase
            || lease.received
        {
            return Ok(false);
        }
        lease.received = true;
        self.changed.notify_all();
        Ok(true)
    }

    pub fn runtime_ready(&self) -> Result<u64, String> {
        let mut data = self.lock_data()?;
        data.runtime_ready_epoch = data
            .runtime_ready_epoch
            .checked_add(1)
            .ok_or_else(|| "window mode runtime ready epoch overflow".to_owned())?;
        let epoch = data.runtime_ready_epoch;
        self.changed.notify_all();
        Ok(epoch)
    }

    #[cfg(test)]
    pub fn wait_runtime_ready_after(&self, epoch: u64, timeout: Duration) -> Result<u64, String> {
        self.wait_runtime_ready(epoch, timeout, None, || {})
    }

    #[allow(dead_code)] // retained for the dormant desktop-mode compatibility path
    fn wait_runtime_ready(
        &self,
        epoch: u64,
        timeout: Duration,
        startup_lease: Option<StartupRestoreLease>,
        before_wait: impl FnOnce(),
    ) -> Result<u64, String> {
        let deadline = Instant::now() + timeout;
        let mut data = self.lock_data()?;
        let mut before_wait = Some(before_wait);
        while data.runtime_ready_epoch <= epoch {
            if startup_lease.is_some_and(|lease| !data.startup_is_current(lease)) {
                return Err(STARTUP_RESTORE_CANCELLED.to_owned());
            }
            if let Some(hook) = before_wait.take() {
                hook();
            }
            if startup_lease.is_some_and(|lease| !data.startup_is_current(lease)) {
                return Err(STARTUP_RESTORE_CANCELLED.to_owned());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("timed out waiting for window mode runtime ready".to_owned());
            }
            let (next, wait) = self
                .changed
                .wait_timeout(data, remaining)
                .map_err(|_| "window mode controller lock poisoned".to_owned())?;
            data = next;
            if wait.timed_out() && data.runtime_ready_epoch <= epoch {
                return Err("timed out waiting for window mode runtime ready".to_owned());
            }
        }
        Ok(data.runtime_ready_epoch)
    }

    #[allow(dead_code)] // retained for the dormant desktop-mode compatibility path
    pub fn begin_startup_restore(&self) -> Result<StartupRestoreLease, String> {
        let mut data = self.lock_data()?;
        if data.startup_active.is_some() {
            return Err("startup window mode restoration is already active".to_owned());
        }
        data.startup_generation = data
            .startup_generation
            .checked_add(1)
            .ok_or_else(|| "startup window mode generation overflow".to_owned())?;
        let lease = StartupRestoreLease(data.startup_generation);
        data.startup_active = Some(lease);
        Ok(lease)
    }

    pub fn cancel_startup_restore_and_wait(&self) -> Result<(), String> {
        let mut data = self.lock_data()?;
        if data.startup_active.is_none() {
            return Ok(());
        }
        data.startup_generation = data
            .startup_generation
            .checked_add(1)
            .ok_or_else(|| "startup window mode generation overflow".to_owned())?;
        self.changed.notify_all();
        while data.startup_active.is_some() {
            data = self
                .changed
                .wait(data)
                .map_err(|_| "window mode controller lock poisoned".to_owned())?;
        }
        Ok(())
    }

    #[allow(dead_code)] // retained for the dormant desktop-mode compatibility path
    fn finish_startup_restore(&self, lease: StartupRestoreLease) -> Result<(), String> {
        let mut data = self.lock_data()?;
        if data.startup_active == Some(lease) {
            data.startup_active = None;
            self.changed.notify_all();
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn restore_saved_mode(
        &self,
        request_id: String,
        saved_mode: WindowMode,
    ) -> Result<WindowModeSnapshot, String> {
        let result = self.restore_saved_mode_inner(request_id, saved_mode);
        self.publish_terminal(result)
    }

    #[cfg(test)]
    fn restore_saved_mode_inner(
        &self,
        request_id: String,
        saved_mode: WindowMode,
    ) -> Result<WindowModeSnapshot, String> {
        if saved_mode == WindowMode::Companion {
            return self.snapshot();
        }
        self.restore_saved_mode_inner_for_startup(request_id, saved_mode, None)
    }

    #[allow(dead_code)] // retained for the dormant desktop-mode compatibility path
    fn restore_saved_mode_inner_for_startup(
        &self,
        request_id: String,
        saved_mode: WindowMode,
        startup_lease: Option<StartupRestoreLease>,
    ) -> Result<WindowModeSnapshot, String> {
        match self.set_mode_inner_with_startup(request_id, saved_mode, startup_lease) {
            Ok(snapshot) => Ok(snapshot),
            Err(root) if root == STARTUP_RESTORE_CANCELLED => Err(root),
            Err(root) => {
                let user_visible = self.snapshot()?.user_visible;
                Err(append_compensation(
                    root,
                    self.io.persist(WindowMode::Companion, user_visible).err(),
                ))
            }
        }
    }

    #[cfg(test)]
    pub fn restore_saved_mode_when_ready(
        &self,
        request_id: String,
        saved_mode: WindowMode,
        ready_timeout: Duration,
    ) -> Result<WindowModeSnapshot, String> {
        let lease = self.begin_startup_restore()?;
        self.restore_startup_mode_when_ready(lease, request_id, saved_mode, ready_timeout)
    }

    #[allow(dead_code)] // retained for the dormant desktop-mode compatibility path
    pub fn restore_startup_mode_when_ready(
        &self,
        lease: StartupRestoreLease,
        request_id: String,
        saved_mode: WindowMode,
        ready_timeout: Duration,
    ) -> Result<WindowModeSnapshot, String> {
        let result = self.restore_saved_mode_when_ready_inner(
            lease,
            request_id,
            saved_mode,
            ready_timeout,
            || {},
        );
        self.finish_startup_restore(lease)?;
        self.publish_terminal(result)
    }

    #[cfg(test)]
    fn restore_saved_mode_when_ready_with_wait_hook(
        &self,
        lease: StartupRestoreLease,
        request_id: String,
        saved_mode: WindowMode,
        ready_timeout: Duration,
        before_wait: impl FnOnce(),
    ) -> Result<WindowModeSnapshot, String> {
        let result = self.restore_saved_mode_when_ready_inner(
            lease,
            request_id,
            saved_mode,
            ready_timeout,
            before_wait,
        );
        self.finish_startup_restore(lease)?;
        self.publish_terminal(result)
    }

    #[allow(dead_code)] // retained for the dormant desktop-mode compatibility path
    fn restore_saved_mode_when_ready_inner(
        &self,
        lease: StartupRestoreLease,
        request_id: String,
        saved_mode: WindowMode,
        ready_timeout: Duration,
        before_wait: impl FnOnce(),
    ) -> Result<WindowModeSnapshot, String> {
        if saved_mode == WindowMode::Companion {
            return self.snapshot();
        }
        if let Err(root) = self.wait_runtime_ready(0, ready_timeout, Some(lease), before_wait) {
            if root == STARTUP_RESTORE_CANCELLED {
                return Err(root);
            }
            let user_visible = self.snapshot()?.user_visible;
            return Err(append_compensation(
                root,
                self.io.persist(WindowMode::Companion, user_visible).err(),
            ));
        }
        self.restore_saved_mode_inner_for_startup(request_id, saved_mode, Some(lease))
    }

    pub fn set_user_visible(&self, visible: bool) -> Result<WindowModeSnapshot, String> {
        let result = self.set_user_visible_inner(visible);
        self.publish_terminal(result)
    }

    fn set_user_visible_inner(&self, visible: bool) -> Result<WindowModeSnapshot, String> {
        let (old_machine, next_machine, mode, degraded, request_id) = {
            let mut data = self.lock_data()?;
            if data.active.is_some() || data.side_operation_in_progress {
                return Err("window mode transition is in progress".to_owned());
            }
            let next = reduce_window_mode(
                data.machine.clone(),
                WindowModeEvent::UserVisibilityChanged(visible),
            )
            .state;
            data.visibility_sequence = data
                .visibility_sequence
                .checked_add(1)
                .ok_or_else(|| "window visibility sequence overflow".to_owned())?;
            let result = (
                data.machine.clone(),
                next,
                data.actual_mode.unwrap_or(WindowMode::Companion),
                data.degraded,
                format!("visibility:user:{}", data.visibility_sequence),
            );
            data.side_operation_in_progress = true;
            result
        };
        let old_visible = visibility(&old_machine) && !degraded;
        let next_visible = visibility(&next_machine) && !degraded;
        let mut next_cycle = 1;
        if let Err(error) =
            self.synchronize_visibility(&request_id, &mut next_cycle, old_visible, next_visible)
        {
            self.lock_data()?.side_operation_in_progress = false;
            self.changed.notify_all();
            return Err(error);
        }
        if let Err(error) = self.io.persist(mode, visible) {
            let compensation = self
                .synchronize_visibility(&request_id, &mut next_cycle, next_visible, old_visible)
                .err();
            self.lock_data()?.side_operation_in_progress = false;
            self.changed.notify_all();
            return Err(append_compensation(error, compensation));
        }
        let mut data = self.lock_data()?;
        data.machine = next_machine;
        data.side_operation_in_progress = false;
        self.changed.notify_all();
        Ok(data.snapshot())
    }

    pub fn fullscreen_changed(&self, active: bool) -> Result<WindowModeSnapshot, String> {
        let result = self.fullscreen_changed_inner(active);
        self.publish_terminal(result)
    }

    fn fullscreen_changed_inner(&self, active: bool) -> Result<WindowModeSnapshot, String> {
        let (old_machine, next_machine, request_id) = {
            let mut data = self.lock_data()?;
            loop {
                match data.active.as_ref() {
                    Some(transition) if transition.pre_commit => {
                        data.pending_precommit_fullscreen = Some(active);
                        return Ok(data.snapshot());
                    }
                    Some(_) => {
                        data = self
                            .changed
                            .wait(data)
                            .map_err(|_| "window mode controller lock poisoned".to_owned())?;
                    }
                    None if data.side_operation_in_progress => {
                        data = self
                            .changed
                            .wait(data)
                            .map_err(|_| "window mode controller lock poisoned".to_owned())?;
                    }
                    None => break,
                }
            }
            let next = reduce_window_mode(
                data.machine.clone(),
                WindowModeEvent::FullscreenChanged(active),
            )
            .state;
            if visibility(&data.machine) == visibility(&next) || data.degraded {
                data.machine = next;
                return Ok(data.snapshot());
            }
            data.visibility_sequence = data
                .visibility_sequence
                .checked_add(1)
                .ok_or_else(|| "window visibility sequence overflow".to_owned())?;
            data.side_operation_in_progress = true;
            (
                data.machine.clone(),
                next,
                format!("visibility:fullscreen:{}", data.visibility_sequence),
            )
        };
        let mut next_cycle = 1;
        if let Err(error) = self.synchronize_visibility(
            &request_id,
            &mut next_cycle,
            visibility(&old_machine),
            visibility(&next_machine),
        ) {
            self.lock_data()?.side_operation_in_progress = false;
            self.changed.notify_all();
            return Err(error);
        }
        let mut data = self.lock_data()?;
        data.machine = next_machine;
        data.side_operation_in_progress = false;
        self.changed.notify_all();
        Ok(data.snapshot())
    }

    fn publish_terminal(
        &self,
        result: Result<WindowModeSnapshot, String>,
    ) -> Result<WindowModeSnapshot, String> {
        let operation_in_progress = self
            .lock_data()
            .map(|data| {
                data.active.is_some() || data.side_operation_in_progress || data.recovery.is_some()
            })
            .unwrap_or(true);
        if operation_in_progress {
            return result;
        }
        let canonical = match self.snapshot() {
            Ok(snapshot) => snapshot,
            Err(_) => return result,
        };
        if let Err(error) = self.io.publish_snapshot(&canonical) {
            self.io
                .report_recovery(&format!("window mode snapshot publication failed: {error}"));
        }
        result.map(|_| canonical)
    }

    fn finish_recovery_lease(&self, _request_id: &str) -> Result<(), String> {
        let mut data = self.lock_data()?;
        data.recovery = None;
        data.active_recovery_generation = None;
        data.side_operation_in_progress = false;
        self.changed.notify_all();
        Ok(())
    }

    fn recover_desktop_host(
        &self,
        request_id: String,
        cancellation: Arc<RecoveryCancellation>,
    ) -> Result<WindowModeSnapshot, String> {
        let (companion_snapshot, was_visible) = {
            let mut data = self.lock_data()?;
            if cancellation.is_cancelled() || data.shutting_down {
                data.recovery = None;
                data.active_recovery_generation = None;
                data.side_operation_in_progress = false;
                self.changed.notify_all();
                return Ok(data.snapshot());
            }
            let Some(snapshot) = data.companion_snapshot else {
                let error = "desktop recovery is missing the companion snapshot".to_owned();
                let generation = data
                    .active_recovery_generation
                    .unwrap_or(data.recovery_generation);
                data.recovery_failure_gate = Some(RecoveryFailureGate {
                    generation,
                    error: error.clone(),
                });
                data.recovery = None;
                data.active_recovery_generation = None;
                data.side_operation_in_progress = false;
                self.changed.notify_all();
                return Err(error);
            };
            let was_visible =
                visibility(&data.machine) && !data.degraded && !data.visibility_degraded;
            data.machine =
                reduce_window_mode(data.machine.clone(), WindowModeEvent::ExplorerLost).state;
            data.actual_mode = None;
            data.desktop_strategy = None;
            data.active = Some(ActiveTransition {
                request_id: request_id.clone(),
                requested_mode: WindowMode::Desktop,
                next_cycle: 1,
                pre_commit: false,
                startup_lease: None,
            });
            data.stamp_canonical();
            (snapshot, was_visible)
        };

        let result = self.execute_desktop_recovery(
            &request_id,
            &cancellation,
            companion_snapshot,
            was_visible,
        );
        let mut data = self.lock_data()?;
        if data
            .active
            .as_ref()
            .is_some_and(|active| active.request_id == request_id)
        {
            data.active = None;
        }
        if data
            .runtime_ack
            .as_ref()
            .is_some_and(|lease| lease.request_id == request_id)
        {
            data.runtime_ack = None;
        }
        data.recovery = None;
        data.active_recovery_generation = None;
        data.side_operation_in_progress = false;
        self.changed.notify_all();
        result
    }

    fn execute_desktop_recovery(
        &self,
        request_id: &str,
        cancellation: &RecoveryCancellation,
        companion_snapshot: WindowHostSnapshot,
        was_visible: bool,
    ) -> Result<WindowModeSnapshot, String> {
        let mut next_cycle = self.next_mode_cycle(request_id)?;
        let mut errors = Vec::new();
        if let Err(error) =
            self.synchronize_visibility(request_id, &mut next_cycle, was_visible, false)
        {
            errors.push(format!("hide lost desktop host failed: {error}"));
            if let Err(hide_error) = self.io.set_visible(false) {
                errors.push(format!("fail-closed hide failed: {hide_error}"));
            }
        }

        for attempt in 0..5 {
            if cancellation.is_cancelled() {
                return self.cancel_desktop_recovery(
                    request_id,
                    &mut next_cycle,
                    companion_snapshot,
                    errors,
                );
            }
            if attempt > 0
                && !self
                    .recovery_wait
                    .wait(DESKTOP_RECOVERY_BACKOFFS[attempt - 1], cancellation)
            {
                return self.cancel_desktop_recovery(
                    request_id,
                    &mut next_cycle,
                    companion_snapshot,
                    errors,
                );
            }
            match self.io.attach_desktop_host(&companion_snapshot) {
                Ok(outcome) => {
                    let machine = {
                        let data = self.lock_data()?;
                        desktop_machine_with_outcome(&data.machine, outcome)
                    };
                    let visible = visibility(&machine);
                    if let Err(error) =
                        self.synchronize_visibility(request_id, &mut next_cycle, false, visible)
                    {
                        errors.push(format!("recovered visibility failed: {error}"));
                        let _ = self.io.set_visible(false);
                        continue;
                    }
                    let mut data = self.lock_data()?;
                    data.machine = machine;
                    data.actual_mode = Some(WindowMode::Desktop);
                    data.desktop_strategy = Some(outcome.into());
                    data.degraded = false;
                    data.visibility_degraded = false;
                    data.runtime_synchronized = true;
                    data.recovery_failure_gate = None;
                    if !errors.is_empty() {
                        self.io.report_recovery(&format!(
                            "desktop host recovered after {} failed step(s): {}",
                            errors.len(),
                            errors.join("; ")
                        ));
                    }
                    return Ok(data.snapshot());
                }
                Err(error) => errors.push(format!("attempt {} failed: {error}", attempt + 1)),
            }
        }

        self.finish_recovery_to_companion(
            request_id,
            &mut next_cycle,
            companion_snapshot,
            errors,
            "Explorer host recovery exhausted",
        )
    }

    fn finish_recovery_to_companion(
        &self,
        request_id: &str,
        next_cycle: &mut u64,
        companion_snapshot: WindowHostSnapshot,
        mut recovery_history: Vec<String>,
        context: &str,
    ) -> Result<WindowModeSnapshot, String> {
        let mut terminal_errors = Vec::new();
        let restore_error = self.io.restore_window_host(&companion_snapshot).err();
        if let Some(error) = &restore_error {
            terminal_errors.push(format!("companion restore failed: {error}"));
        }
        let companion_machine = {
            let data = self.lock_data()?;
            safe_companion_machine(&data.machine)
        };
        let companion_visible = visibility(&companion_machine);
        let mut visibility_failed = false;
        if restore_error.is_none() {
            if let Err(error) =
                self.synchronize_visibility(request_id, next_cycle, false, companion_visible)
            {
                terminal_errors.push(format!("companion visibility failed: {error}"));
                visibility_failed = true;
            }
        }
        let persist_error = self
            .io
            .persist(WindowMode::Companion, companion_machine.user_visible())
            .err();
        if let Some(error) = &persist_error {
            terminal_errors.push(format!("companion persistence failed: {error}"));
            if restore_error.is_none() && !visibility_failed && companion_visible {
                if let Err(hide_error) =
                    self.synchronize_visibility(request_id, next_cycle, true, false)
                {
                    terminal_errors.push(format!(
                        "persist failure fail-closed hide failed: {hide_error}"
                    ));
                    visibility_failed = true;
                }
            }
        }
        recovery_history.extend(terminal_errors.iter().cloned());
        let message = if terminal_errors.is_empty() {
            format!(
                "{context}; companion intent selected: {}",
                recovery_history.join("; ")
            )
        } else {
            format!(
                "{context}; companion intent selected but current host recovery failed: {}",
                recovery_history.join("; ")
            )
        };
        self.io.report_recovery(&message);
        let mut data = self.lock_data()?;
        data.machine = companion_machine;
        data.desired_mode = WindowMode::Companion;
        data.desktop_strategy = None;
        if terminal_errors.is_empty() {
            data.actual_mode = Some(WindowMode::Companion);
            data.companion_snapshot = None;
            data.degraded = false;
            data.visibility_degraded = false;
            data.runtime_synchronized = true;
            data.recovery_failure_gate = None;
            Ok(data.snapshot())
        } else {
            data.actual_mode = None;
            data.companion_snapshot = Some(companion_snapshot);
            data.degraded = true;
            if visibility_failed {
                data.visibility_degraded = true;
                data.runtime_synchronized = false;
            }
            data.stamp_canonical();
            let error = format!(
                "{context}; companion intent selected but current host recovery failed: {}",
                terminal_errors.join("; ")
            );
            let generation = data
                .active_recovery_generation
                .unwrap_or(data.recovery_generation);
            data.recovery_failure_gate = Some(RecoveryFailureGate {
                generation,
                error: error.clone(),
            });
            Err(error)
        }
    }

    fn cancel_desktop_recovery(
        &self,
        request_id: &str,
        next_cycle: &mut u64,
        companion_snapshot: WindowHostSnapshot,
        mut errors: Vec<String>,
    ) -> Result<WindowModeSnapshot, String> {
        if self.lock_data()?.shutting_down {
            let cancellation_failed = match self.io.restore_window_host(&companion_snapshot) {
                Ok(()) => false,
                Err(error) => {
                    errors.push(format!("cancel restore failed: {error}"));
                    true
                }
            };
            let mut data = self.lock_data()?;
            data.desktop_strategy = None;
            data.runtime_synchronized = false;
            if cancellation_failed {
                data.actual_mode = None;
                data.companion_snapshot = Some(companion_snapshot);
            } else {
                data.actual_mode = Some(WindowMode::Companion);
                data.companion_snapshot = None;
            }
            if !errors.is_empty() {
                self.io.report_recovery(&format!(
                    "shutdown recovery cancellation had failures: {}",
                    errors.join("; ")
                ));
            }
            return Ok(data.snapshot());
        }
        self.finish_recovery_to_companion(
            request_id,
            next_cycle,
            companion_snapshot,
            errors,
            "desktop recovery cancelled by a manual mode request",
        )
    }

    fn synchronize_visibility(
        &self,
        request_id: &str,
        next_cycle: &mut u64,
        old_visible: bool,
        new_visible: bool,
    ) -> Result<(), String> {
        if old_visible == new_visible {
            return Ok(());
        }
        let cycle = take_cycle(next_cycle)?;
        if let Err(root) =
            self.begin_runtime_phase(request_id, cycle, RuntimeAckPhase::Paused, None)
        {
            let _ = self.best_effort_runtime_restore(request_id, cycle, old_visible);
            self.set_runtime_synchronized(false)?;
            return Err(root);
        }
        if let Err(root) = self.io.set_visible(new_visible) {
            let mut compensation_errors = Vec::new();
            if let Err(error) = self.io.set_visible(old_visible) {
                compensation_errors.push(error);
                self.set_visibility_degraded(true)?;
            } else {
                self.set_visibility_degraded(false)?;
            }
            if let Err(error) = self.begin_runtime_phase(
                request_id,
                cycle,
                RuntimeAckPhase::Resumed,
                Some(old_visible),
            ) {
                compensation_errors.push(error);
                self.set_runtime_synchronized(false)?;
            } else {
                self.set_runtime_synchronized(true)?;
            }
            return Err(append_compensations(root, compensation_errors));
        }
        if let Err(root) = self.begin_runtime_phase(
            request_id,
            cycle,
            RuntimeAckPhase::Resumed,
            Some(new_visible),
        ) {
            let compensation = self.compensate_visibility(request_id, next_cycle, old_visible);
            if compensation.is_err() {
                self.set_visibility_degraded(true)?;
                self.set_runtime_synchronized(false)?;
            } else {
                self.set_visibility_degraded(false)?;
                self.set_runtime_synchronized(true)?;
            }
            return Err(append_compensation(root, compensation.err()));
        }
        self.set_visibility_degraded(false)?;
        self.set_runtime_synchronized(true)?;
        Ok(())
    }

    fn compensate_visibility(
        &self,
        request_id: &str,
        next_cycle: &mut u64,
        old_visible: bool,
    ) -> Result<(), String> {
        let cycle = take_cycle(next_cycle)?;
        self.begin_runtime_phase(request_id, cycle, RuntimeAckPhase::Paused, None)?;
        self.io.set_visible(old_visible)?;
        self.begin_runtime_phase(
            request_id,
            cycle,
            RuntimeAckPhase::Resumed,
            Some(old_visible),
        )
    }

    fn execute_runtime_recovery(
        &self,
        request_id: &str,
        requested_mode: WindowMode,
        old: &TransitionBaseline,
    ) -> Result<WindowModeSnapshot, String> {
        let cycle = self.next_mode_cycle(request_id)?;
        self.begin_runtime_phase(request_id, cycle, RuntimeAckPhase::Paused, None)?;
        let effective_visible =
            visibility(&old.machine) && !old.degraded && !old.visibility_degraded;
        self.begin_runtime_phase(
            request_id,
            cycle,
            RuntimeAckPhase::Resumed,
            Some(effective_visible),
        )?;
        let mut data = self.lock_data()?;
        data.runtime_synchronized = true;
        let snapshot = data.snapshot();
        data.active = None;
        data.cache_completion(request_id.to_owned(), requested_mode, Ok(snapshot.clone()));
        self.changed.notify_all();
        Ok(snapshot)
    }

    fn best_effort_runtime_restore(
        &self,
        request_id: &str,
        cycle: u64,
        visible: bool,
    ) -> Option<String> {
        self.io
            .emit_runtime(request_id, cycle, RuntimeAckPhase::Resumed, Some(visible))
            .err()
    }

    fn set_visibility_degraded(&self, degraded: bool) -> Result<(), String> {
        let mut data = self.lock_data()?;
        data.visibility_degraded = degraded;
        data.stamp_canonical();
        Ok(())
    }

    fn set_runtime_synchronized(&self, synchronized: bool) -> Result<(), String> {
        let mut data = self.lock_data()?;
        data.runtime_synchronized = synchronized;
        data.stamp_canonical();
        Ok(())
    }

    fn execute_transition(
        &self,
        request_id: &str,
        requested_mode: WindowMode,
        old: &TransitionBaseline,
    ) -> Result<WindowModeSnapshot, String> {
        let mut renderer_paused = false;
        let mut paused_cycle = None;
        let mut pause_attempted_cycle = None;
        let mut physical_mutated = false;
        let mut preference_persisted = false;
        let mut new_companion_snapshot = old.companion_snapshot;
        let mut new_strategy = None;

        let attempt = (|| -> Result<WindowModeSnapshot, String> {
            let cycle = self.next_mode_cycle(request_id)?;
            pause_attempted_cycle = Some(cycle);
            self.begin_runtime_phase(request_id, cycle, RuntimeAckPhase::Paused, None)?;
            renderer_paused = true;
            paused_cycle = Some(cycle);

            let mut machine = self.lock_data()?.machine.clone();
            match requested_mode {
                WindowMode::Desktop => {
                    let captured = self.io.capture_window_host()?;
                    new_companion_snapshot = Some(captured);
                    // The adapter may have partially mutated the HWND before reporting an
                    // attach error. Treat the attempt as a mutation so controller-level
                    // compensation remains a second, idempotent line of defence.
                    physical_mutated = true;
                    let outcome = self.io.attach_desktop_host(&captured)?;
                    machine = match outcome {
                        DesktopAttachOutcome::WorkerW { parent } => {
                            reduce_window_mode(
                                machine,
                                WindowModeEvent::HostAttached(DesktopHostState::WorkerW { parent }),
                            )
                            .state
                        }
                        DesktopAttachOutcome::BottomFallback => {
                            let fallback = reduce_window_mode(
                                machine,
                                WindowModeEvent::HostFailed(DesktopHostAttempt::WorkerW),
                            )
                            .state;
                            reduce_window_mode(
                                fallback,
                                WindowModeEvent::HostAttached(DesktopHostState::BottomFallback),
                            )
                            .state
                        }
                    };
                    new_strategy = Some(outcome.into());
                }
                WindowMode::Companion => {
                    let snapshot = old.companion_snapshot.ok_or_else(|| {
                        "cannot restore companion mode without a captured window host".to_owned()
                    })?;
                    physical_mutated = true;
                    self.io.restore_window_host(&snapshot)?;
                    new_companion_snapshot = None;
                }
            }

            machine = self.drain_precommit_fullscreen(request_id, machine)?;

            let mut finished = if machine.transition().is_some() {
                reduce_window_mode(machine, WindowModeEvent::TransitionFinished(requested_mode))
                    .state
            } else if old.degraded && requested_mode == WindowMode::Companion {
                machine
            } else {
                return Err("window mode reducer rejected transition completion".to_owned());
            };
            if finished.transition().is_some() {
                return Err("window mode reducer rejected transition completion".to_owned());
            }
            let mut effective_visible = visibility(&finished);
            self.io.set_visible(effective_visible)?;
            self.begin_runtime_phase(
                request_id,
                cycle,
                RuntimeAckPhase::Resumed,
                Some(effective_visible),
            )?;
            renderer_paused = false;
            paused_cycle = None;
            loop {
                let pending = {
                    let mut data = self.lock_data()?;
                    let active = data
                        .active
                        .as_ref()
                        .ok_or_else(|| "window mode transition lease disappeared".to_owned())?;
                    if active.request_id != request_id {
                        return Err("window mode transition lease changed".to_owned());
                    }
                    match data.pending_precommit_fullscreen.take() {
                        Some(fullscreen) => Some(fullscreen),
                        None => {
                            data.active
                                .as_mut()
                                .expect("active transition validated above")
                                .pre_commit = false;
                            None
                        }
                    }
                };
                let Some(pending) = pending else {
                    break;
                };
                let next = reduce_window_mode(
                    finished.clone(),
                    WindowModeEvent::FullscreenChanged(pending),
                )
                .state;
                let next_visible = visibility(&next);
                if next_visible != effective_visible {
                    let cycle = self.next_mode_cycle(request_id)?;
                    self.begin_runtime_phase(request_id, cycle, RuntimeAckPhase::Paused, None)?;
                    renderer_paused = true;
                    paused_cycle = Some(cycle);
                    self.io.set_visible(next_visible)?;
                    self.begin_runtime_phase(
                        request_id,
                        cycle,
                        RuntimeAckPhase::Resumed,
                        Some(next_visible),
                    )?;
                    renderer_paused = false;
                    paused_cycle = None;
                    effective_visible = next_visible;
                }
                finished = next;
                let mut data = self.lock_data()?;
                data.machine = finished.clone();
                data.stamp_canonical();
            }
            self.io.persist(requested_mode, finished.user_visible())?;
            preference_persisted = true;
            let mut data = self.lock_data()?;
            data.machine = finished;
            data.desired_mode = requested_mode;
            data.actual_mode = Some(requested_mode);
            data.degraded = false;
            data.visibility_degraded = false;
            data.runtime_synchronized = true;
            data.desktop_strategy = new_strategy;
            data.companion_snapshot = new_companion_snapshot;
            let snapshot = data.snapshot();
            data.active = None;
            data.cache_completion(request_id.to_owned(), requested_mode, Ok(snapshot.clone()));
            self.changed.notify_all();
            Ok(snapshot)
        })();

        match attempt {
            Ok(snapshot) => Ok(snapshot),
            Err(root) => {
                if !physical_mutated && !renderer_paused {
                    let runtime_restore_error = pause_attempted_cycle.and_then(|cycle| {
                        self.best_effort_runtime_restore(
                            request_id,
                            cycle,
                            visibility(&old.machine) && !old.degraded && !old.visibility_degraded,
                        )
                    });
                    let mut data = self.lock_data()?;
                    data.machine = old.machine.clone();
                    data.desired_mode = old.desired_mode;
                    data.actual_mode = old.actual_mode;
                    data.degraded = old.degraded;
                    data.visibility_degraded = old.visibility_degraded;
                    data.runtime_synchronized = false;
                    data.desktop_strategy = old.desktop_strategy;
                    data.companion_snapshot = old.companion_snapshot;
                    data.pending_precommit_fullscreen = None;
                    data.stamp_canonical();
                    return Err(append_compensation(root, runtime_restore_error));
                }
                let mut compensation_errors = Vec::new();
                if physical_mutated && !renderer_paused {
                    match self.next_mode_cycle(request_id).and_then(|cycle| {
                        self.begin_runtime_phase(request_id, cycle, RuntimeAckPhase::Paused, None)?;
                        Ok(cycle)
                    }) {
                        Ok(cycle) => {
                            renderer_paused = true;
                            paused_cycle = Some(cycle);
                        }
                        Err(error) => compensation_errors.push(error),
                    }
                }
                let mut compensated = renderer_paused || physical_mutated;
                let mut compensated_strategy = old.desktop_strategy;
                let mut compensated_machine = old.machine.clone();
                if physical_mutated {
                    match old.actual_mode {
                        Some(WindowMode::Companion) => {
                            if let Some(snapshot) = new_companion_snapshot {
                                if let Err(error) = self.io.restore_window_host(&snapshot) {
                                    compensation_errors.push(error);
                                    compensated = false;
                                }
                            } else {
                                compensation_errors.push(
                                    "missing companion snapshot for host compensation".to_owned(),
                                );
                                compensated = false;
                            }
                            compensated_strategy = None;
                        }
                        Some(WindowMode::Desktop) => {
                            if let Some(snapshot) = old.companion_snapshot {
                                match self.io.attach_desktop_host(&snapshot) {
                                    Ok(outcome) => {
                                        compensated_strategy = Some(outcome.into());
                                        compensated_machine =
                                            desktop_machine_with_outcome(&old.machine, outcome);
                                    }
                                    Err(error) => {
                                        compensation_errors.push(error);
                                        compensated = false;
                                    }
                                }
                            } else {
                                compensation_errors.push(
                                    "missing desktop baseline snapshot for host compensation"
                                        .to_owned(),
                                );
                                compensated = false;
                            }
                        }
                        None => compensated = false,
                    }
                }
                match self.drain_precommit_fullscreen(request_id, compensated_machine.clone()) {
                    Ok(machine) => compensated_machine = machine,
                    Err(error) => {
                        compensation_errors.push(error);
                        compensated = false;
                    }
                }
                let compensated_visible =
                    compensated && visibility(&compensated_machine) && !old.degraded;
                if let Err(error) = self.io.set_visible(compensated_visible) {
                    compensation_errors.push(error);
                    compensated = false;
                }
                if renderer_paused {
                    let cycle = paused_cycle
                        .ok_or_else(|| "window mode renderer pause cycle disappeared".to_owned());
                    if let Err(error) = cycle.and_then(|cycle| {
                        self.begin_runtime_phase(
                            request_id,
                            cycle,
                            RuntimeAckPhase::Resumed,
                            Some(compensated_visible),
                        )
                    }) {
                        compensation_errors.push(error);
                        compensated = false;
                    }
                }
                if preference_persisted {
                    let old_mode = old.actual_mode.unwrap_or(WindowMode::Companion);
                    if let Err(error) = self.io.persist(old_mode, old.machine.user_visible()) {
                        compensation_errors.push(error);
                        compensated = false;
                    }
                }
                if !compensated {
                    if let Err(error) = self.io.set_visible(false) {
                        compensation_errors.push(error);
                    }
                }
                let mut data = self.lock_data()?;
                if compensated {
                    data.machine = compensated_machine;
                    data.desired_mode = old.desired_mode;
                    data.actual_mode = old.actual_mode;
                    data.degraded = old.degraded;
                    data.visibility_degraded = old.visibility_degraded;
                    data.runtime_synchronized = true;
                    data.desktop_strategy = compensated_strategy;
                    data.companion_snapshot = old.companion_snapshot;
                } else {
                    data.machine = safe_companion_machine(&old.machine);
                    data.desired_mode = old.desired_mode;
                    data.actual_mode = None;
                    data.degraded = true;
                    data.visibility_degraded = true;
                    data.runtime_synchronized = false;
                    data.desktop_strategy = None;
                    data.companion_snapshot = new_companion_snapshot.or(old.companion_snapshot);
                }
                data.stamp_canonical();
                Err(append_compensations(root, compensation_errors))
            }
        }
    }

    fn next_mode_cycle(&self, request_id: &str) -> Result<u64, String> {
        let mut data = self.lock_data()?;
        let active = data
            .active
            .as_mut()
            .ok_or_else(|| "window mode transition lease disappeared".to_owned())?;
        if active.request_id != request_id {
            return Err("window mode transition lease changed".to_owned());
        }
        let cycle = active.next_cycle;
        active.next_cycle = active
            .next_cycle
            .checked_add(1)
            .ok_or_else(|| "window mode runtime cycle overflow".to_owned())?;
        Ok(cycle)
    }

    fn begin_runtime_phase(
        &self,
        request_id: &str,
        cycle: u64,
        phase: RuntimeAckPhase,
        effective_visible: Option<bool>,
    ) -> Result<(), String> {
        {
            let mut data = self.lock_data()?;
            data.runtime_ack = Some(RuntimeAckLease {
                request_id: request_id.to_owned(),
                cycle,
                phase,
                received: false,
            });
            self.changed.notify_all();
        }
        self.io
            .emit_runtime(request_id, cycle, phase, effective_visible)?;
        self.wait_for_ack(request_id, cycle, phase)
    }

    fn drain_precommit_fullscreen(
        &self,
        request_id: &str,
        mut machine: WindowModeState,
    ) -> Result<WindowModeState, String> {
        loop {
            let pending = {
                let mut data = self.lock_data()?;
                let active_request_id = data
                    .active
                    .as_ref()
                    .map(|active| active.request_id.as_str())
                    .ok_or_else(|| "window mode transition lease disappeared".to_owned())?;
                if active_request_id != request_id {
                    return Err("window mode transition lease changed".to_owned());
                }
                match data.pending_precommit_fullscreen.take() {
                    Some(fullscreen) => Some(fullscreen),
                    None => None,
                }
            };
            let Some(fullscreen) = pending else {
                return Ok(machine);
            };
            machine =
                reduce_window_mode(machine, WindowModeEvent::FullscreenChanged(fullscreen)).state;
            let mut data = self.lock_data()?;
            data.machine = machine.clone();
            data.stamp_canonical();
        }
    }

    fn wait_for_ack(
        &self,
        request_id: &str,
        cycle: u64,
        phase: RuntimeAckPhase,
    ) -> Result<(), String> {
        let deadline = Instant::now() + self.ack_timeout;
        let mut data = self.lock_data()?;
        loop {
            if data.active.as_ref().is_some_and(|active| {
                active
                    .startup_lease
                    .is_some_and(|lease| !data.startup_is_current(lease))
            }) {
                data.runtime_ack = None;
                return Err(STARTUP_RESTORE_CANCELLED.to_owned());
            }
            let lease = data
                .runtime_ack
                .as_ref()
                .ok_or_else(|| "window mode runtime ACK lease disappeared".to_owned())?;
            if lease.request_id != request_id || lease.cycle != cycle || lease.phase != phase {
                return Err("window mode runtime ACK lease changed".to_owned());
            }
            if lease.received {
                data.runtime_ack = None;
                return Ok(());
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(format!(
                    "timed out waiting for window mode runtime {} ACK",
                    phase.as_str()
                ));
            }
            let remaining = deadline.saturating_duration_since(now);
            let (next, timeout) = self
                .changed
                .wait_timeout(data, remaining)
                .map_err(|_| "window mode controller lock poisoned".to_owned())?;
            data = next;
            if timeout.timed_out() {
                return Err(format!(
                    "timed out waiting for window mode runtime {} ACK",
                    phase.as_str()
                ));
            }
        }
    }

    fn lock_data(&self) -> Result<MutexGuard<'_, ControllerData>, String> {
        self.data
            .lock()
            .map_err(|_| "window mode controller lock poisoned".to_owned())
    }
}

#[derive(Clone)]
struct TransitionBaseline {
    machine: WindowModeState,
    desired_mode: WindowMode,
    actual_mode: Option<WindowMode>,
    degraded: bool,
    visibility_degraded: bool,
    desktop_strategy: Option<DesktopStrategy>,
    companion_snapshot: Option<WindowHostSnapshot>,
}

fn safe_companion_machine(source: &WindowModeState) -> WindowModeState {
    if source.desired_mode() == WindowMode::Companion && source.transition().is_none() {
        return source.clone();
    }
    let requested = reduce_window_mode(
        source.clone(),
        WindowModeEvent::RequestMode(WindowMode::Companion),
    )
    .state;
    reduce_window_mode(
        requested,
        WindowModeEvent::TransitionFinished(WindowMode::Companion),
    )
    .state
}

fn desktop_machine_with_outcome(
    source: &WindowModeState,
    outcome: DesktopAttachOutcome,
) -> WindowModeState {
    let companion = safe_companion_machine(source);
    let mut desktop =
        reduce_window_mode(companion, WindowModeEvent::RequestMode(WindowMode::Desktop)).state;
    desktop = match outcome {
        DesktopAttachOutcome::WorkerW { parent } => {
            reduce_window_mode(
                desktop,
                WindowModeEvent::HostAttached(DesktopHostState::WorkerW { parent }),
            )
            .state
        }
        DesktopAttachOutcome::BottomFallback => {
            let fallback = reduce_window_mode(
                desktop,
                WindowModeEvent::HostFailed(DesktopHostAttempt::WorkerW),
            )
            .state;
            reduce_window_mode(
                fallback,
                WindowModeEvent::HostAttached(DesktopHostState::BottomFallback),
            )
            .state
        }
    };
    reduce_window_mode(
        desktop,
        WindowModeEvent::TransitionFinished(WindowMode::Desktop),
    )
    .state
}

fn desktop_outcome(
    machine: &WindowModeState,
    strategy: Option<DesktopStrategy>,
) -> Option<DesktopAttachOutcome> {
    match (strategy, machine.desktop_host()) {
        (Some(DesktopStrategy::WorkerW), DesktopHostState::WorkerW { parent }) if parent != 0 => {
            Some(DesktopAttachOutcome::WorkerW { parent })
        }
        (Some(DesktopStrategy::BottomFallback), DesktopHostState::BottomFallback) => {
            Some(DesktopAttachOutcome::BottomFallback)
        }
        _ => None,
    }
}

fn visibility(state: &WindowModeState) -> bool {
    state.visibility_action() == WindowModeAction::Show
}

fn take_cycle(next_cycle: &mut u64) -> Result<u64, String> {
    let cycle = *next_cycle;
    *next_cycle = (*next_cycle)
        .checked_add(1)
        .ok_or_else(|| "window mode runtime cycle overflow".to_owned())?;
    Ok(cycle)
}

fn append_compensation(root: String, compensation: Option<String>) -> String {
    match compensation {
        Some(compensation) => format!("{root}; compensation failed: {compensation}"),
        None => root,
    }
}

fn append_compensations(root: String, compensation_errors: Vec<String>) -> String {
    if compensation_errors.is_empty() {
        root
    } else {
        format!(
            "{root}; compensation failed: {}",
            compensation_errors.join("; ")
        )
    }
}

pub fn validate_request_id(request_id: &str) -> Result<(), String> {
    if request_id.is_empty()
        || request_id.len() > MAX_REQUEST_ID_BYTES
        || !request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(
            "window mode requestId must be 1-128 ASCII letters, digits, '.', ':', '_' or '-'"
                .to_owned(),
        );
    }
    Ok(())
}

pub fn canonical_public_mode(_: WindowMode) -> WindowMode {
    WindowMode::Companion
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::VecDeque,
        sync::atomic::{AtomicBool, Ordering},
        thread,
    };

    const SNAPSHOT: WindowHostSnapshot = WindowHostSnapshot {
        parent: 0,
        style: 1,
        ex_style: 2,
        rect: crate::platform::ScreenRect {
            left: 1,
            top: 2,
            right: 101,
            bottom: 202,
        },
        topmost: true,
        z_order_after: 7,
    };

    #[derive(Default)]
    struct FakeState {
        events: Vec<FakeRuntimeEvent>,
        emit_results: VecDeque<Result<(), String>>,
        visible: Vec<bool>,
        physical_visible: Option<bool>,
        persisted: Vec<(WindowMode, bool)>,
        captures: usize,
        attaches: usize,
        restores: usize,
        attach_error: Option<String>,
        restore_error: Option<String>,
        persist_error: Option<String>,
        outcome: Option<DesktopAttachOutcome>,
        attach_results: VecDeque<Result<DesktopAttachOutcome, String>>,
        visible_results: VecDeque<Result<(), String>>,
        operations: Vec<String>,
        restore_gate: Option<Arc<(Mutex<bool>, Condvar)>>,
        host_alive_results: VecDeque<Result<bool, String>>,
        recovery_notices: Vec<String>,
        published: Vec<WindowModeSnapshot>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct FakeRuntimeEvent {
        request_id: String,
        cycle: u64,
        phase: RuntimeAckPhase,
        effective_visible: Option<bool>,
    }

    #[derive(Default)]
    struct FakeIo(Mutex<FakeState>);

    impl FakeIo {
        fn state(&self) -> MutexGuard<'_, FakeState> {
            self.0.lock().unwrap()
        }
    }

    impl WindowModeIo for FakeIo {
        fn emit_runtime(
            &self,
            request_id: &str,
            cycle: u64,
            phase: RuntimeAckPhase,
            effective_visible: Option<bool>,
        ) -> Result<(), String> {
            let mut state = self.state();
            state.operations.push(format!("emit:{}", phase.as_str()));
            if let Some(result) = state.emit_results.pop_front() {
                result?;
            }
            state.events.push(FakeRuntimeEvent {
                request_id: request_id.to_owned(),
                cycle,
                phase,
                effective_visible,
            });
            Ok(())
        }
        fn capture_window_host(&self) -> Result<WindowHostSnapshot, String> {
            self.state().captures += 1;
            Ok(SNAPSHOT)
        }
        fn attach_desktop_host(
            &self,
            _: &WindowHostSnapshot,
        ) -> Result<DesktopAttachOutcome, String> {
            let mut state = self.state();
            state.attaches += 1;
            if let Some(result) = state.attach_results.pop_front() {
                return result;
            }
            if let Some(error) = &state.attach_error {
                return Err(error.clone());
            }
            Ok(state
                .outcome
                .unwrap_or(DesktopAttachOutcome::WorkerW { parent: 99 }))
        }
        fn restore_window_host(&self, _: &WindowHostSnapshot) -> Result<(), String> {
            let (gate, error) = {
                let mut state = self.state();
                state.restores += 1;
                (state.restore_gate.clone(), state.restore_error.clone())
            };
            if let Some(gate) = gate {
                let (lock, changed) = &*gate;
                let mut released = lock.lock().unwrap();
                while !*released {
                    released = changed.wait(released).unwrap();
                }
            }
            if let Some(error) = error {
                return Err(error.clone());
            }
            Ok(())
        }
        fn set_visible(&self, visible: bool) -> Result<(), String> {
            let mut state = self.state();
            state.visible.push(visible);
            state.physical_visible = Some(visible);
            state.operations.push(format!("visible:{visible}"));
            state.visible_results.pop_front().unwrap_or(Ok(()))
        }
        fn persist(&self, mode: WindowMode, user_visible: bool) -> Result<(), String> {
            let mut state = self.state();
            if let Some(error) = &state.persist_error {
                return Err(error.clone());
            }
            state.persisted.push((mode, user_visible));
            state
                .operations
                .push(format!("persist:{mode:?}:{user_visible}"));
            Ok(())
        }
        fn desktop_host_alive(&self, _: DesktopAttachOutcome) -> Result<bool, String> {
            self.state()
                .host_alive_results
                .pop_front()
                .unwrap_or(Ok(true))
        }
        fn publish_snapshot(&self, snapshot: &WindowModeSnapshot) -> Result<(), String> {
            self.state().published.push(snapshot.clone());
            Ok(())
        }
        fn report_recovery(&self, message: &str) {
            self.state().recovery_notices.push(message.to_owned());
        }
    }

    #[derive(Default)]
    struct FakeRecoveryWait(Mutex<Vec<Duration>>);

    impl RecoveryWait for FakeRecoveryWait {
        fn wait(&self, delay: Duration, cancellation: &RecoveryCancellation) -> bool {
            self.0.lock().unwrap().push(delay);
            !cancellation.is_cancelled()
        }
    }

    struct BlockingRecoveryWait {
        started: Arc<(Mutex<bool>, Condvar)>,
    }

    struct CoordinatedRecoveryWait {
        started: Arc<(Mutex<bool>, Condvar)>,
        release: Arc<(Mutex<bool>, Condvar)>,
    }

    impl RecoveryWait for CoordinatedRecoveryWait {
        fn wait(&self, _: Duration, cancellation: &RecoveryCancellation) -> bool {
            let (started, changed) = &*self.started;
            *started.lock().unwrap() = true;
            changed.notify_all();
            let mut cancelled = cancellation.cancelled.lock().unwrap();
            while !*cancelled {
                cancelled = cancellation.changed.wait(cancelled).unwrap();
            }
            drop(cancelled);
            let (release, changed) = &*self.release;
            let mut released = release.lock().unwrap();
            while !*released {
                released = changed.wait(released).unwrap();
            }
            false
        }
    }

    impl RecoveryWait for BlockingRecoveryWait {
        fn wait(&self, _: Duration, cancellation: &RecoveryCancellation) -> bool {
            let (started, changed) = &*self.started;
            *started.lock().unwrap() = true;
            changed.notify_all();
            let mut cancelled = cancellation.cancelled.lock().unwrap();
            while !*cancelled {
                cancelled = cancellation.changed.wait(cancelled).unwrap();
            }
            false
        }
    }

    fn cancel_failure_harness(
        configure: impl FnOnce(&mut FakeState),
    ) -> (
        Arc<WindowModeController>,
        Arc<FakeIo>,
        thread::JoinHandle<()>,
    ) {
        let io = Arc::new(FakeIo::default());
        let started = Arc::new((Mutex::new(false), Condvar::new()));
        let controller = Arc::new(WindowModeController::with_timeout_and_recovery_wait(
            io.clone(),
            true,
            Duration::from_millis(300),
            Arc::new(BlockingRecoveryWait {
                started: started.clone(),
            }),
        ));
        let initial_ack = auto_ack(controller.clone(), io.clone());
        controller
            .set_mode(
                "desktop-before-manual-cancel-matrix".into(),
                WindowMode::Desktop,
            )
            .unwrap();
        initial_ack.join().unwrap();
        {
            let mut state = io.state();
            state.host_alive_results.push_back(Ok(false));
            state
                .attach_results
                .push_back(Err("Explorer still starting".into()));
            state.events.clear();
            configure(&mut state);
        }
        let recovery_ack = auto_ack(controller.clone(), io.clone());
        let recovery = {
            let controller = controller.clone();
            thread::spawn(move || controller.check_desktop_host())
        };
        let (lock, changed) = &*started;
        let mut reached_wait = lock.lock().unwrap();
        while !*reached_wait {
            reached_wait = changed.wait(reached_wait).unwrap();
        }
        drop(reached_wait);
        (
            controller,
            io,
            thread::spawn(move || {
                recovery.join().unwrap().unwrap_err();
                recovery_ack.join().unwrap();
            }),
        )
    }

    fn controller(io: Arc<FakeIo>) -> Arc<WindowModeController> {
        Arc::new(WindowModeController::with_timeout(
            io,
            true,
            Duration::from_millis(300),
        ))
    }

    fn auto_ack(controller: Arc<WindowModeController>, io: Arc<FakeIo>) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let mut acknowledged = 0;
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                let events = io.state().events.clone();
                while acknowledged < events.len() {
                    let event = &events[acknowledged];
                    let _ = controller.runtime_ack(&event.request_id, event.cycle, event.phase);
                    acknowledged += 1;
                }
                let data = controller.lock_data().unwrap();
                if data.active.is_none()
                    && data.runtime_ack.is_none()
                    && !data.side_operation_in_progress
                    && acknowledged > 0
                {
                    return;
                }
                drop(data);
                thread::sleep(Duration::from_millis(1));
            }
        })
    }

    #[test]
    fn double_attach_failure_restores_companion_and_does_not_persist_desktop() {
        let io = Arc::new(FakeIo::default());
        io.state().attach_error = Some("WorkerW failed; bottom fallback failed".into());
        let controller = controller(io.clone());
        let ack = auto_ack(controller.clone(), io.clone());
        let error = controller
            .set_mode("req-1".into(), WindowMode::Desktop)
            .unwrap_err();
        ack.join().unwrap();
        assert!(error.contains("WorkerW"));
        assert_eq!(
            controller.snapshot().unwrap().actual_mode,
            Some(WindowMode::Companion)
        );
        assert!(io.state().persisted.is_empty());
        assert_eq!(io.state().restores, 1);
    }

    #[test]
    fn public_mode_contract_normalizes_legacy_desktop_to_companion() {
        assert_eq!(
            canonical_public_mode(WindowMode::Desktop),
            WindowMode::Companion
        );
        assert_eq!(
            canonical_public_mode(WindowMode::Companion),
            WindowMode::Companion
        );
    }

    #[test]
    fn successful_transition_waits_for_phase_bound_ack_then_persists() {
        let io = Arc::new(FakeIo::default());
        let controller = controller(io.clone());
        let done = Arc::new(AtomicBool::new(false));
        let worker = {
            let controller = controller.clone();
            let done = done.clone();
            thread::spawn(move || {
                let result = controller.set_mode("bound-1".into(), WindowMode::Desktop);
                done.store(true, Ordering::SeqCst);
                result
            })
        };
        while io.state().events.is_empty() {
            thread::yield_now();
        }
        assert!(!controller
            .runtime_ack("wrong", 1, RuntimeAckPhase::Paused)
            .unwrap());
        assert!(!controller
            .runtime_ack("bound-1", 1, RuntimeAckPhase::Resumed)
            .unwrap());
        assert!(!controller
            .runtime_ack("bound-1", 2, RuntimeAckPhase::Paused)
            .unwrap());
        assert!(!done.load(Ordering::SeqCst));
        assert!(controller
            .runtime_ack("bound-1", 1, RuntimeAckPhase::Paused)
            .unwrap());
        assert!(!controller
            .runtime_ack("bound-1", 1, RuntimeAckPhase::Paused)
            .unwrap());
        while io.state().events.len() < 2 {
            thread::yield_now();
        }
        assert!(controller
            .runtime_ack("bound-1", 1, RuntimeAckPhase::Resumed)
            .unwrap());
        let snapshot = worker.join().unwrap().unwrap();
        assert_eq!(snapshot.actual_mode, Some(WindowMode::Desktop));
        assert_eq!(snapshot.revision, 2);
        assert_eq!(io.state().published.last(), Some(&snapshot));
        assert_eq!(io.state().persisted, vec![(WindowMode::Desktop, true)]);
    }

    #[test]
    fn different_request_is_rejected_while_same_request_waits_and_reuses_result() {
        let io = Arc::new(FakeIo::default());
        let controller = controller(io.clone());
        let first = {
            let controller = controller.clone();
            thread::spawn(move || controller.set_mode("shared-1".into(), WindowMode::Desktop))
        };
        while io.state().events.is_empty() {
            thread::yield_now();
        }
        assert!(controller
            .set_mode("other-1".into(), WindowMode::Desktop)
            .unwrap_err()
            .contains("in progress"));
        let same = {
            let controller = controller.clone();
            thread::spawn(move || controller.set_mode("shared-1".into(), WindowMode::Desktop))
        };
        assert!(controller
            .set_mode("shared-1".into(), WindowMode::Companion)
            .unwrap_err()
            .contains("bound"));
        assert!(controller
            .runtime_ack("shared-1", 1, RuntimeAckPhase::Paused)
            .unwrap());
        while io.state().events.len() < 2 {
            thread::yield_now();
        }
        assert!(controller
            .runtime_ack("shared-1", 1, RuntimeAckPhase::Resumed)
            .unwrap());
        assert_eq!(
            first.join().unwrap().unwrap(),
            same.join().unwrap().unwrap()
        );
        assert_eq!(io.state().captures, 1);
        assert_eq!(io.state().attaches, 1);
    }

    #[test]
    fn unsafe_request_id_is_rejected_before_any_effect() {
        let io = Arc::new(FakeIo::default());
        let controller = controller(io.clone());
        let error = controller
            .set_mode("../escape".into(), WindowMode::Desktop)
            .unwrap_err();
        assert!(error.contains("requestId"));
        assert!(io.state().events.is_empty());
    }

    #[test]
    fn startup_desktop_failure_persists_companion_and_preserves_hidden_intent() {
        let io = Arc::new(FakeIo::default());
        io.state().attach_error = Some("WorkerW and fallback unavailable".into());
        let controller = Arc::new(WindowModeController::with_timeout(
            io.clone(),
            false,
            Duration::from_millis(300),
        ));
        let ack = auto_ack(controller.clone(), io.clone());
        let error = controller
            .restore_saved_mode("startup-mode-restore".into(), WindowMode::Desktop)
            .unwrap_err();
        ack.join().unwrap();
        assert!(error.contains("WorkerW"));
        assert_eq!(io.state().persisted, vec![(WindowMode::Companion, false)]);
        let snapshot = controller.snapshot().unwrap();
        assert_eq!(snapshot.actual_mode, Some(WindowMode::Companion));
        assert!(!snapshot.user_visible);
    }

    #[test]
    fn companion_pause_timeout_preserves_known_mode_and_idempotent_request_recovers_runtime() {
        let io = Arc::new(FakeIo::default());
        let controller = Arc::new(WindowModeController::with_timeout(
            io.clone(),
            true,
            Duration::from_millis(10),
        ));
        let error = controller
            .set_mode("timeout-1".into(), WindowMode::Desktop)
            .unwrap_err();
        assert!(error.contains("paused ACK"));
        assert_eq!(io.state().captures, 0);
        assert!(io.state().persisted.is_empty());
        let snapshot = controller.snapshot().unwrap();
        assert_eq!(snapshot.actual_mode, Some(WindowMode::Companion));
        assert_eq!(snapshot.desired_mode, WindowMode::Companion);
        assert!(!snapshot
            .suppressions
            .contains(&SuppressionReason::Transition));
        assert_eq!(
            io.state()
                .events
                .last()
                .map(|event| (event.phase, event.effective_visible)),
            Some((RuntimeAckPhase::Resumed, Some(true)))
        );

        io.state().events.clear();
        let ack = auto_ack(controller.clone(), io.clone());
        let recovered = controller
            .set_mode("companion-runtime-recovery".into(), WindowMode::Companion)
            .unwrap();
        ack.join().unwrap();
        assert_eq!(recovered.actual_mode, Some(WindowMode::Companion));
        assert_eq!(
            io.state()
                .events
                .iter()
                .map(|event| event.phase)
                .collect::<Vec<_>>(),
            vec![RuntimeAckPhase::Paused, RuntimeAckPhase::Resumed]
        );
    }

    #[test]
    fn companion_pause_emit_failure_preserves_known_mode_without_physical_work() {
        let io = Arc::new(FakeIo::default());
        io.state()
            .emit_results
            .push_back(Err("pause emit failed".into()));
        let controller = controller(io.clone());

        let error = controller
            .set_mode("emit-fails".into(), WindowMode::Desktop)
            .unwrap_err();

        assert!(error.starts_with("pause emit failed"));
        assert_eq!(
            controller.snapshot().unwrap().actual_mode,
            Some(WindowMode::Companion)
        );
        assert_eq!(io.state().captures, 0);
        assert!(io.state().persisted.is_empty());
        assert_eq!(
            io.state()
                .events
                .last()
                .map(|event| (event.phase, event.effective_visible)),
            Some((RuntimeAckPhase::Resumed, Some(true)))
        );
    }

    #[test]
    fn desktop_pause_timeout_preserves_host_and_a_later_companion_request_succeeds() {
        let io = Arc::new(FakeIo::default());
        let controller = Arc::new(WindowModeController::with_timeout(
            io.clone(),
            true,
            Duration::from_millis(20),
        ));
        let first_ack = auto_ack(controller.clone(), io.clone());
        controller
            .set_mode("desktop-before-timeout".into(), WindowMode::Desktop)
            .unwrap();
        first_ack.join().unwrap();
        io.state().events.clear();

        let error = controller
            .set_mode("companion-times-out".into(), WindowMode::Companion)
            .unwrap_err();
        assert!(error.contains("paused ACK"));
        let snapshot = controller.snapshot().unwrap();
        assert_eq!(snapshot.actual_mode, Some(WindowMode::Desktop));
        assert_eq!(snapshot.desired_mode, WindowMode::Desktop);
        assert_eq!(snapshot.desktop_strategy, Some(DesktopStrategy::WorkerW));
        assert_eq!(io.state().restores, 0);

        io.state().events.clear();
        let recovery_ack = auto_ack(controller.clone(), io.clone());
        let recovered = controller
            .set_mode("companion-after-timeout".into(), WindowMode::Companion)
            .unwrap();
        recovery_ack.join().unwrap();
        assert_eq!(recovered.actual_mode, Some(WindowMode::Companion));
    }

    #[test]
    fn persist_failure_restores_old_host_and_keeps_root_error_first() {
        let io = Arc::new(FakeIo::default());
        io.state().persist_error = Some("disk full".into());
        io.state().restore_error = Some("restore style failed".into());
        let controller = controller(io.clone());
        let ack = auto_ack(controller.clone(), io.clone());
        let error = controller
            .set_mode("save-1".into(), WindowMode::Desktop)
            .unwrap_err();
        ack.join().unwrap();
        assert!(error.starts_with("disk full"));
        assert!(error.contains("compensation failed: restore style failed"));
        assert_eq!(controller.snapshot().unwrap().actual_mode, None);
    }

    #[test]
    fn companion_persist_failure_rehosts_the_previous_desktop_mode() {
        let io = Arc::new(FakeIo::default());
        let controller = controller(io.clone());
        let first_ack = auto_ack(controller.clone(), io.clone());
        controller
            .set_mode("desktop-first".into(), WindowMode::Desktop)
            .unwrap();
        first_ack.join().unwrap();
        io.state().events.clear();
        io.state().persist_error = Some("disk full switching companion".into());

        let second_ack = auto_ack(controller.clone(), io.clone());
        let error = controller
            .set_mode("companion-fails".into(), WindowMode::Companion)
            .unwrap_err();
        second_ack.join().unwrap();

        assert!(error.starts_with("disk full switching companion"));
        let snapshot = controller.snapshot().unwrap();
        assert_eq!(snapshot.actual_mode, Some(WindowMode::Desktop));
        assert_eq!(snapshot.desired_mode, WindowMode::Desktop);
        assert_eq!(snapshot.desktop_strategy, Some(DesktopStrategy::WorkerW));
        assert_eq!(io.state().restores, 1);
        assert_eq!(io.state().attaches, 2);
        assert_eq!(
            io.state()
                .events
                .iter()
                .map(|event| (event.cycle, event.phase, event.effective_visible))
                .collect::<Vec<_>>(),
            vec![
                (1, RuntimeAckPhase::Paused, None),
                (1, RuntimeAckPhase::Resumed, Some(true)),
                (2, RuntimeAckPhase::Paused, None),
                (2, RuntimeAckPhase::Resumed, Some(true)),
            ]
        );
    }

    #[test]
    fn visibility_intent_is_persisted_without_reading_physical_visibility() {
        let io = Arc::new(FakeIo::default());
        let controller = controller(io.clone());
        let ack = auto_ack(controller.clone(), io.clone());
        let snapshot = controller.set_user_visible(false).unwrap();
        ack.join().unwrap();
        assert!(!snapshot.user_visible);
        assert_eq!(io.state().visible, vec![false]);
        assert_eq!(io.state().persisted, vec![(WindowMode::Companion, false)]);
        assert_eq!(io.state().events[1].effective_visible, Some(false));
    }

    #[test]
    fn visibility_error_restores_the_old_native_value_before_resuming_renderer() {
        let io = Arc::new(FakeIo::default());
        io.state()
            .visible_results
            .push_back(Err("hide mutated then failed".into()));
        io.state().visible_results.push_back(Ok(()));
        let controller = controller(io.clone());
        let ack = auto_ack(controller.clone(), io.clone());

        let error = controller.set_user_visible(false).unwrap_err();
        ack.join().unwrap();

        assert!(error.starts_with("hide mutated then failed"));
        assert_eq!(io.state().visible, vec![false, true]);
        assert!(io
            .state()
            .events
            .iter()
            .all(|event| validate_request_id(&event.request_id).is_ok()));
        assert_eq!(io.state().physical_visible, Some(true));
        let snapshot = controller.snapshot().unwrap();
        assert!(snapshot.user_visible);
        assert_eq!(snapshot.actual_mode, Some(WindowMode::Companion));
        assert!(!snapshot
            .suppressions
            .contains(&SuppressionReason::Transition));
        assert_eq!(
            io.state().events.last().unwrap().effective_visible,
            Some(true)
        );
    }

    #[test]
    fn failed_visibility_restore_keeps_known_host_but_reports_degraded_state() {
        let io = Arc::new(FakeIo::default());
        io.state()
            .visible_results
            .push_back(Err("hide mutated then failed".into()));
        io.state()
            .visible_results
            .push_back(Err("show restore failed".into()));
        let controller = controller(io.clone());
        let ack = auto_ack(controller.clone(), io.clone());

        let error = controller.set_user_visible(false).unwrap_err();
        ack.join().unwrap();

        assert!(error.starts_with("hide mutated then failed"));
        assert!(error.contains("compensation failed: show restore failed"));
        assert_eq!(io.state().visible, vec![false, true]);
        let snapshot = controller.snapshot().unwrap();
        assert_eq!(snapshot.actual_mode, Some(WindowMode::Companion));
        assert!(snapshot
            .suppressions
            .contains(&SuppressionReason::Transition));
    }

    #[test]
    fn fullscreen_hides_companion_but_does_not_change_user_intent() {
        let io = Arc::new(FakeIo::default());
        let controller = controller(io.clone());
        let ack = auto_ack(controller.clone(), io.clone());
        let snapshot = controller.fullscreen_changed(true).unwrap();
        ack.join().unwrap();
        assert!(snapshot.user_visible);
        assert!(snapshot
            .suppressions
            .contains(&SuppressionReason::Fullscreen));
        assert_eq!(io.state().visible, vec![false]);
        assert!(io.state().persisted.is_empty());
        assert_eq!(io.state().events[1].effective_visible, Some(false));

        io.state().events.clear();
        controller.fullscreen_changed(true).unwrap();
        assert!(io.state().events.is_empty());
    }

    #[test]
    fn fullscreen_observed_during_transition_is_applied_before_returning_companion() {
        let io = Arc::new(FakeIo::default());
        let controller = controller(io.clone());
        let first_ack = auto_ack(controller.clone(), io.clone());
        controller
            .set_mode("desktop-before-fullscreen".into(), WindowMode::Desktop)
            .unwrap();
        first_ack.join().unwrap();
        io.state().events.clear();

        let transition = {
            let controller = controller.clone();
            thread::spawn(move || {
                controller.set_mode("companion-in-fullscreen".into(), WindowMode::Companion)
            })
        };
        while io.state().events.is_empty() {
            thread::yield_now();
        }
        controller.fullscreen_changed(true).unwrap();
        assert!(controller
            .runtime_ack("companion-in-fullscreen", 1, RuntimeAckPhase::Paused)
            .unwrap());
        while io.state().events.len() < 2 {
            thread::yield_now();
        }
        assert!(controller
            .runtime_ack("companion-in-fullscreen", 1, RuntimeAckPhase::Resumed)
            .unwrap());

        let snapshot = transition.join().unwrap().unwrap();
        assert_eq!(snapshot.actual_mode, Some(WindowMode::Companion));
        assert!(snapshot
            .suppressions
            .contains(&SuppressionReason::Fullscreen));
        assert_eq!(io.state().visible.last(), Some(&false));
    }

    #[test]
    fn runtime_ready_is_an_explicit_epoch_handshake() {
        let io = Arc::new(FakeIo::default());
        let controller = controller(io);
        let waiting = {
            let controller = controller.clone();
            thread::spawn(move || {
                controller.wait_runtime_ready_after(0, Duration::from_millis(200))
            })
        };
        thread::sleep(Duration::from_millis(20));
        assert!(!waiting.is_finished());
        assert_eq!(controller.runtime_ready().unwrap(), 1);
        assert_eq!(waiting.join().unwrap().unwrap(), 1);
        assert_eq!(controller.runtime_ready().unwrap(), 2);
    }

    #[test]
    fn every_public_mode_terminal_publishes_its_canonical_snapshot() {
        let io = Arc::new(FakeIo::default());
        let controller = controller(io.clone());

        let success = controller
            .set_mode("publish-companion".into(), WindowMode::Companion)
            .unwrap();
        assert_eq!(success.revision, 0);
        assert_eq!(io.state().published, vec![success]);

        let error = controller
            .set_mode("unsafe request id".into(), WindowMode::Desktop)
            .unwrap_err();
        assert!(error.contains("requestId"));
        let published = &io.state().published;
        assert_eq!(published.len(), 2);
        assert_eq!(published.last().unwrap(), &controller.snapshot().unwrap());
        assert_eq!(published.last().unwrap().revision, 0);
    }

    #[test]
    fn startup_desktop_restore_waits_for_runtime_ready() {
        let io = Arc::new(FakeIo::default());
        let controller = controller(io.clone());
        let ack = auto_ack(controller.clone(), io.clone());
        let worker = {
            let controller = controller.clone();
            thread::spawn(move || {
                controller.restore_saved_mode_when_ready(
                    "startup-ready".into(),
                    WindowMode::Desktop,
                    Duration::from_millis(200),
                )
            })
        };
        thread::sleep(Duration::from_millis(20));
        assert!(io.state().events.is_empty());
        controller.runtime_ready().unwrap();
        assert_eq!(
            worker.join().unwrap().unwrap().actual_mode,
            Some(WindowMode::Desktop)
        );
        ack.join().unwrap();
    }

    #[test]
    fn startup_cancel_and_wait_cannot_lose_wake_between_check_and_condvar_wait() {
        let io = Arc::new(FakeIo::default());
        let controller = controller(io);
        let lease = controller.begin_startup_restore().unwrap();
        let entered = Arc::new((Mutex::new(false), Condvar::new()));
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let startup = {
            let controller = controller.clone();
            let entered = entered.clone();
            let release = release.clone();
            thread::spawn(move || {
                controller.restore_saved_mode_when_ready_with_wait_hook(
                    lease,
                    "startup-no-lost-wake".into(),
                    WindowMode::Desktop,
                    Duration::from_secs(1),
                    || {
                        let (lock, changed) = &*entered;
                        *lock.lock().unwrap() = true;
                        changed.notify_all();
                        let (lock, changed) = &*release;
                        let mut released = lock.lock().unwrap();
                        while !*released {
                            released = changed.wait(released).unwrap();
                        }
                    },
                )
            })
        };
        {
            let (lock, changed) = &*entered;
            let mut value = lock.lock().unwrap();
            while !*value {
                value = changed.wait(value).unwrap();
            }
        }
        let explicit = {
            let controller = controller.clone();
            thread::spawn(move || controller.cancel_startup_restore_and_wait())
        };
        {
            let (lock, changed) = &*release;
            *lock.lock().unwrap() = true;
            changed.notify_all();
        }

        assert!(startup.join().unwrap().unwrap_err().contains("cancelled"));
        explicit.join().unwrap().unwrap();
    }

    #[test]
    fn startup_cancel_interrupts_pending_ack_before_physical_work_and_explicit_mode_wins() {
        let io = Arc::new(FakeIo::default());
        let controller = controller(io.clone());
        let lease = controller.begin_startup_restore().unwrap();
        controller.runtime_ready().unwrap();
        let startup = {
            let controller = controller.clone();
            thread::spawn(move || {
                controller.restore_startup_mode_when_ready(
                    lease,
                    "startup-ack-cancel".into(),
                    WindowMode::Desktop,
                    Duration::from_secs(1),
                )
            })
        };
        {
            let mut data = controller.lock_data().unwrap();
            while data.runtime_ack.is_none() {
                data = controller.changed.wait(data).unwrap();
            }
        }
        let explicit = {
            let controller = controller.clone();
            thread::spawn(move || {
                controller.cancel_startup_restore_and_wait().unwrap();
                controller.set_mode("explicit-after-startup".into(), WindowMode::Companion)
            })
        };

        assert!(startup.join().unwrap().unwrap_err().contains("cancelled"));
        let explicit_ack = auto_ack(controller.clone(), io.clone());
        assert_eq!(
            explicit.join().unwrap().unwrap().actual_mode,
            Some(WindowMode::Companion)
        );
        explicit_ack.join().unwrap();
        assert_eq!(io.state().attaches, 0);
        assert!(io.state().persisted.is_empty());
    }

    #[test]
    fn canonical_mutation_is_revision_stamped_before_the_controller_unlocks() {
        let io = Arc::new(FakeIo::default());
        let controller = controller(io.clone());
        let before = controller.snapshot().unwrap();
        let ack = auto_ack(controller.clone(), io);

        let result = controller
            .set_mode_inner("revision-before-publish".into(), WindowMode::Desktop)
            .unwrap();
        ack.join().unwrap();
        let concurrent_get = controller.snapshot().unwrap();

        assert_eq!(concurrent_get.actual_mode, Some(WindowMode::Desktop));
        assert!(concurrent_get.revision > before.revision);
        assert_eq!(result, concurrent_get);
        assert_eq!(controller.snapshot().unwrap(), concurrent_get);
    }

    #[test]
    fn startup_ready_timeout_persists_safe_companion_and_hidden_intent() {
        let io = Arc::new(FakeIo::default());
        let controller =
            WindowModeController::with_timeout(io.clone(), false, Duration::from_millis(20));
        let error = controller
            .restore_saved_mode_when_ready(
                "startup-timeout".into(),
                WindowMode::Desktop,
                Duration::from_millis(10),
            )
            .unwrap_err();
        assert!(error.contains("runtime ready"));
        assert!(io.state().events.is_empty());
        assert_eq!(io.state().persisted, vec![(WindowMode::Companion, false)]);
    }

    #[test]
    fn completion_cache_is_bounded_and_still_binds_payload() {
        let io = Arc::new(FakeIo::default());
        let controller = controller(io);
        for index in 0..129 {
            controller
                .set_mode(format!("cached-{index}"), WindowMode::Companion)
                .unwrap();
        }
        let data = controller.lock_data().unwrap();
        assert_eq!(data.completed.len(), 128);
        assert!(!data.completed.contains_key("cached-0"));
        assert!(data.completed.contains_key("cached-128"));
        drop(data);
        assert!(controller
            .set_mode("cached-128".into(), WindowMode::Desktop)
            .unwrap_err()
            .contains("bound"));
    }

    #[test]
    fn desktop_compensation_records_the_real_bottom_fallback_strategy() {
        let io = Arc::new(FakeIo::default());
        let controller = controller(io.clone());
        let first_ack = auto_ack(controller.clone(), io.clone());
        controller
            .set_mode("desktop-workerw".into(), WindowMode::Desktop)
            .unwrap();
        first_ack.join().unwrap();
        io.state()
            .attach_results
            .push_back(Ok(DesktopAttachOutcome::BottomFallback));
        io.state().persist_error = Some("disk full".into());

        let second_ack = auto_ack(controller.clone(), io.clone());
        controller
            .set_mode("companion-rollback".into(), WindowMode::Companion)
            .unwrap_err();
        second_ack.join().unwrap();

        let snapshot = controller.snapshot().unwrap();
        assert_eq!(snapshot.actual_mode, Some(WindowMode::Desktop));
        assert_eq!(
            snapshot.desktop_strategy,
            Some(DesktopStrategy::BottomFallback)
        );
        assert_eq!(
            controller.lock_data().unwrap().machine.desktop_host(),
            DesktopHostState::BottomFallback
        );
    }

    #[test]
    fn failed_host_compensation_reports_unknown_actual_and_stays_suppressed() {
        let io = Arc::new(FakeIo::default());
        let controller = controller(io.clone());
        let first_ack = auto_ack(controller.clone(), io.clone());
        controller
            .set_mode("desktop-before-degraded".into(), WindowMode::Desktop)
            .unwrap();
        first_ack.join().unwrap();
        io.state()
            .attach_results
            .push_back(Err("cannot reattach".into()));
        io.state().persist_error = Some("disk full".into());

        let second_ack = auto_ack(controller.clone(), io.clone());
        let error = controller
            .set_mode("companion-degraded".into(), WindowMode::Companion)
            .unwrap_err();
        second_ack.join().unwrap();

        assert!(error.contains("cannot reattach"));
        let snapshot = controller.snapshot().unwrap();
        assert_eq!(snapshot.actual_mode, None);
        assert!(snapshot
            .suppressions
            .contains(&SuppressionReason::Transition));
        assert_eq!(io.state().visible.last(), Some(&false));
    }

    #[test]
    fn visibility_failure_rolls_back_the_host_before_any_persist() {
        let io = Arc::new(FakeIo::default());
        io.state()
            .visible_results
            .push_back(Err("show desktop failed".into()));
        let controller = controller(io.clone());
        let ack = auto_ack(controller.clone(), io.clone());
        let error = controller
            .set_mode("desktop-visibility-fails".into(), WindowMode::Desktop)
            .unwrap_err();
        ack.join().unwrap();

        assert!(error.starts_with("show desktop failed"));
        assert_eq!(
            controller.snapshot().unwrap().actual_mode,
            Some(WindowMode::Companion)
        );
        assert_eq!(io.state().restores, 1);
        assert!(io.state().persisted.is_empty());
    }

    #[test]
    fn pending_fullscreen_is_applied_before_resume_and_persist() {
        let io = Arc::new(FakeIo::default());
        let controller = controller(io.clone());
        let first_ack = auto_ack(controller.clone(), io.clone());
        controller
            .set_mode(
                "desktop-before-gated-fullscreen".into(),
                WindowMode::Desktop,
            )
            .unwrap();
        first_ack.join().unwrap();

        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        {
            let mut state = io.state();
            state.events.clear();
            state.operations.clear();
            state.restore_gate = Some(gate.clone());
        }
        let worker = {
            let controller = controller.clone();
            thread::spawn(move || {
                controller.set_mode("companion-gated".into(), WindowMode::Companion)
            })
        };
        while io.state().events.is_empty() {
            thread::yield_now();
        }
        controller
            .runtime_ack("companion-gated", 1, RuntimeAckPhase::Paused)
            .unwrap();
        while io.state().restores < 1 {
            thread::yield_now();
        }
        controller.fullscreen_changed(true).unwrap();
        {
            let (released, changed) = &*gate;
            *released.lock().unwrap() = true;
            changed.notify_all();
        }
        while io.state().events.len() < 2 {
            thread::yield_now();
        }
        controller
            .runtime_ack("companion-gated", 1, RuntimeAckPhase::Resumed)
            .unwrap();
        let snapshot = worker.join().unwrap().unwrap();
        let operations = io.state().operations.clone();
        let hidden = operations
            .iter()
            .position(|item| item == "visible:false")
            .unwrap();
        let resumed = operations
            .iter()
            .position(|item| item == "emit:resumed")
            .unwrap();
        let persisted = operations
            .iter()
            .position(|item| item == "persist:Companion:true")
            .unwrap();
        assert!(hidden < resumed && resumed < persisted, "{operations:?}");
        assert!(snapshot
            .suppressions
            .contains(&SuppressionReason::Fullscreen));
    }

    #[test]
    fn pending_fullscreen_is_applied_before_compensation_resume() {
        let io = Arc::new(FakeIo::default());
        io.state().attach_error = Some("host attach failed".into());
        let controller = controller(io.clone());
        let worker = {
            let controller = controller.clone();
            thread::spawn(move || {
                controller.set_mode("desktop-host-fails-fullscreen".into(), WindowMode::Desktop)
            })
        };
        while io.state().events.is_empty() {
            thread::yield_now();
        }
        controller.fullscreen_changed(true).unwrap();
        controller
            .runtime_ack("desktop-host-fails-fullscreen", 1, RuntimeAckPhase::Paused)
            .unwrap();
        while io.state().events.len() < 2 {
            thread::yield_now();
        }
        let operations_before_resume_ack = io.state().operations.clone();
        let hidden = operations_before_resume_ack
            .iter()
            .position(|item| item == "visible:false")
            .unwrap();
        let resumed = operations_before_resume_ack
            .iter()
            .position(|item| item == "emit:resumed")
            .unwrap();
        assert!(hidden < resumed, "{operations_before_resume_ack:?}");
        controller
            .runtime_ack("desktop-host-fails-fullscreen", 1, RuntimeAckPhase::Resumed)
            .unwrap();
        worker.join().unwrap().unwrap_err();
        assert!(controller
            .snapshot()
            .unwrap()
            .suppressions
            .contains(&SuppressionReason::Fullscreen));
    }

    #[test]
    fn leaving_fullscreen_does_not_show_a_manually_hidden_companion() {
        let io = Arc::new(FakeIo::default());
        let controller = controller(io.clone());
        let ack = auto_ack(controller.clone(), io.clone());
        controller.set_user_visible(false).unwrap();
        ack.join().unwrap();
        controller.fullscreen_changed(true).unwrap();
        controller.fullscreen_changed(false).unwrap();
        assert!(!controller.snapshot().unwrap().user_visible);
        assert!(!io.state().visible.iter().any(|visible| *visible));
    }

    #[test]
    fn fullscreen_after_mode_commit_is_an_independent_failure() {
        let io = Arc::new(FakeIo::default());
        let controller = controller(io.clone());
        let first_ack = auto_ack(controller.clone(), io.clone());
        controller
            .set_mode("desktop-before-postcommit".into(), WindowMode::Desktop)
            .unwrap();
        first_ack.join().unwrap();
        let second_ack = auto_ack(controller.clone(), io.clone());
        let committed = controller
            .set_mode("companion-committed".into(), WindowMode::Companion)
            .unwrap();
        second_ack.join().unwrap();
        assert_eq!(committed.actual_mode, Some(WindowMode::Companion));
        let persisted_before = io.state().persisted.clone();

        io.state()
            .visible_results
            .push_back(Err("postcommit hide failed".into()));
        let fullscreen_ack = auto_ack(controller.clone(), io.clone());
        let error = controller.fullscreen_changed(true).unwrap_err();
        fullscreen_ack.join().unwrap();

        assert!(error.starts_with("postcommit hide failed"));
        assert_eq!(io.state().persisted, persisted_before);
        let snapshot = controller.snapshot().unwrap();
        assert_eq!(snapshot.actual_mode, Some(WindowMode::Companion));
        assert!(!snapshot
            .suppressions
            .contains(&SuppressionReason::Fullscreen));
    }

    #[test]
    fn explorer_loss_hides_then_reattaches_without_changing_user_intent() {
        let io = Arc::new(FakeIo::default());
        let wait = Arc::new(FakeRecoveryWait::default());
        let controller = Arc::new(WindowModeController::with_timeout_and_recovery_wait(
            io.clone(),
            true,
            Duration::from_millis(300),
            wait.clone(),
        ));
        let initial_ack = auto_ack(controller.clone(), io.clone());
        controller
            .set_mode("desktop-before-explorer-loss".into(), WindowMode::Desktop)
            .unwrap();
        initial_ack.join().unwrap();
        {
            let mut state = io.state();
            state.host_alive_results.push_back(Ok(false));
            state
                .attach_results
                .push_back(Err("first recovery attempt failed".into()));
            state
                .attach_results
                .push_back(Ok(DesktopAttachOutcome::BottomFallback));
            state.visible.clear();
            state.events.clear();
        }

        let recovery_ack = auto_ack(controller.clone(), io.clone());
        controller.check_desktop_host().unwrap();
        recovery_ack.join().unwrap();

        let snapshot = controller.snapshot().unwrap();
        assert_eq!(snapshot.desired_mode, WindowMode::Desktop);
        assert_eq!(snapshot.actual_mode, Some(WindowMode::Desktop));
        assert_eq!(
            snapshot.desktop_strategy,
            Some(DesktopStrategy::BottomFallback)
        );
        assert!(!snapshot
            .suppressions
            .contains(&SuppressionReason::ExplorerLost));
        assert_eq!(io.state().visible, vec![false, true]);
        assert_eq!(wait.0.lock().unwrap().as_slice(), &[Duration::from_secs(1)]);
        assert_eq!(
            io.state().persisted.last(),
            Some(&(WindowMode::Desktop, true))
        );
    }

    #[test]
    fn explorer_recovery_uses_five_immediate_then_backoff_attempts_before_companion_fallback() {
        let io = Arc::new(FakeIo::default());
        let wait = Arc::new(FakeRecoveryWait::default());
        let controller = Arc::new(WindowModeController::with_timeout_and_recovery_wait(
            io.clone(),
            true,
            Duration::from_millis(300),
            wait.clone(),
        ));
        let initial_ack = auto_ack(controller.clone(), io.clone());
        controller
            .set_mode(
                "desktop-before-terminal-recovery".into(),
                WindowMode::Desktop,
            )
            .unwrap();
        initial_ack.join().unwrap();
        {
            let mut state = io.state();
            state.host_alive_results.push_back(Ok(false));
            state.attach_error = Some("Explorer host unavailable".into());
            state.attaches = 0;
            state.events.clear();
        }

        let recovery_ack = auto_ack(controller.clone(), io.clone());
        controller.check_desktop_host().unwrap();
        recovery_ack.join().unwrap();

        assert_eq!(io.state().attaches, 5);
        assert_eq!(
            wait.0.lock().unwrap().as_slice(),
            &[
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
            ]
        );
        let snapshot = controller.snapshot().unwrap();
        assert_eq!(snapshot.desired_mode, WindowMode::Companion);
        assert_eq!(snapshot.actual_mode, Some(WindowMode::Companion));
        assert_eq!(
            io.state().persisted.last(),
            Some(&(WindowMode::Companion, true))
        );
        assert!(io
            .state()
            .recovery_notices
            .iter()
            .any(|message| message.contains("companion")));
    }

    #[test]
    fn terminal_companion_fallback_reports_persistence_failure_without_claiming_desktop() {
        let io = Arc::new(FakeIo::default());
        let wait = Arc::new(FakeRecoveryWait::default());
        let controller = Arc::new(WindowModeController::with_timeout_and_recovery_wait(
            io.clone(),
            true,
            Duration::from_millis(300),
            wait,
        ));
        let initial_ack = auto_ack(controller.clone(), io.clone());
        controller
            .set_mode(
                "desktop-before-fallback-persist-failure".into(),
                WindowMode::Desktop,
            )
            .unwrap();
        initial_ack.join().unwrap();
        {
            let mut state = io.state();
            state.host_alive_results.push_back(Ok(false));
            state.attach_error = Some("Explorer unavailable".into());
            state.persist_error = Some("disk full".into());
            state.events.clear();
        }

        let recovery_ack = auto_ack(controller.clone(), io.clone());
        let error = controller.check_desktop_host().unwrap_err();
        recovery_ack.join().unwrap();

        assert!(error.contains("persistence failed: disk full"));
        let snapshot = controller.snapshot().unwrap();
        assert_eq!(snapshot.actual_mode, None);
        assert_eq!(snapshot.desired_mode, WindowMode::Companion);
        assert_eq!(snapshot.desktop_strategy, None);
        assert!(snapshot
            .suppressions
            .contains(&SuppressionReason::Transition));
        let data = controller.lock_data().unwrap();
        assert!(data.degraded);
        assert!(data.companion_snapshot.is_some());
        assert_eq!(io.state().physical_visible, Some(false));
    }

    #[test]
    fn terminal_restore_failure_keeps_snapshot_degraded_and_shutdown_retries_restore() {
        let io = Arc::new(FakeIo::default());
        let controller = Arc::new(WindowModeController::with_timeout_and_recovery_wait(
            io.clone(),
            true,
            Duration::from_millis(300),
            Arc::new(FakeRecoveryWait::default()),
        ));
        let initial_ack = auto_ack(controller.clone(), io.clone());
        controller
            .set_mode(
                "desktop-before-terminal-restore-failure".into(),
                WindowMode::Desktop,
            )
            .unwrap();
        initial_ack.join().unwrap();
        {
            let mut state = io.state();
            state.host_alive_results.push_back(Ok(false));
            state.attach_error = Some("Explorer unavailable".into());
            state.restore_error = Some("restore snapshot failed".into());
            state.events.clear();
        }
        let recovery_ack = auto_ack(controller.clone(), io.clone());

        let error = controller.check_desktop_host().unwrap_err();
        recovery_ack.join().unwrap();

        assert!(error.contains("restore snapshot failed"));
        let snapshot = controller.snapshot().unwrap();
        assert_eq!(snapshot.actual_mode, None);
        assert_eq!(snapshot.desired_mode, WindowMode::Companion);
        assert!(snapshot
            .suppressions
            .contains(&SuppressionReason::Transition));
        {
            let data = controller.lock_data().unwrap();
            assert!(data.degraded);
            assert!(data.companion_snapshot.is_some());
        }
        assert_eq!(
            io.state().persisted.last(),
            Some(&(WindowMode::Companion, true))
        );
        io.state().host_alive_results.push_back(Ok(false));
        let still_unknown = controller.check_desktop_host().unwrap();
        assert_eq!(still_unknown.actual_mode, None);
        assert_eq!(io.state().host_alive_results.len(), 1);

        io.state().restore_error = None;
        controller.shutdown();
        assert_eq!(io.state().restores, 2);
    }

    #[test]
    fn terminal_native_visibility_failure_is_unknown_and_runtime_unsynchronized() {
        let io = Arc::new(FakeIo::default());
        let controller = Arc::new(WindowModeController::with_timeout_and_recovery_wait(
            io.clone(),
            true,
            Duration::from_millis(300),
            Arc::new(FakeRecoveryWait::default()),
        ));
        let initial_ack = auto_ack(controller.clone(), io.clone());
        controller
            .set_mode(
                "desktop-before-terminal-visibility-failure".into(),
                WindowMode::Desktop,
            )
            .unwrap();
        initial_ack.join().unwrap();
        {
            let mut state = io.state();
            state.host_alive_results.push_back(Ok(false));
            state.attach_error = Some("Explorer unavailable".into());
            state.visible_results.push_back(Ok(()));
            state
                .visible_results
                .push_back(Err("show companion failed after mutation".into()));
            state
                .visible_results
                .push_back(Err("hide compensation failed".into()));
            state.events.clear();
        }
        let recovery_ack = auto_ack(controller.clone(), io.clone());

        let error = controller.check_desktop_host().unwrap_err();
        recovery_ack.join().unwrap();

        assert!(error.contains("show companion failed after mutation"));
        assert!(error.contains("hide compensation failed"));
        let data = controller.lock_data().unwrap();
        assert_eq!(data.actual_mode, None);
        assert!(data.degraded);
        assert!(data.visibility_degraded);
        assert!(!data.runtime_synchronized);
        assert!(data.companion_snapshot.is_some());
    }

    #[test]
    fn terminal_runtime_ack_failure_keeps_host_unknown_even_when_companion_persists() {
        let io = Arc::new(FakeIo::default());
        let controller = Arc::new(WindowModeController::with_timeout_and_recovery_wait(
            io.clone(),
            true,
            Duration::from_millis(300),
            Arc::new(FakeRecoveryWait::default()),
        ));
        let initial_ack = auto_ack(controller.clone(), io.clone());
        controller
            .set_mode(
                "desktop-before-terminal-ack-failure".into(),
                WindowMode::Desktop,
            )
            .unwrap();
        initial_ack.join().unwrap();
        {
            let mut state = io.state();
            state.host_alive_results.push_back(Ok(false));
            state.attach_error = Some("Explorer unavailable".into());
            state.emit_results.push_back(Ok(()));
            state.emit_results.push_back(Ok(()));
            state
                .emit_results
                .push_back(Err("terminal pause emit failed".into()));
            state.events.clear();
        }
        let recovery_ack = auto_ack(controller.clone(), io.clone());

        let error = controller.check_desktop_host().unwrap_err();
        recovery_ack.join().unwrap();

        assert!(error.contains("terminal pause emit failed"));
        assert_eq!(
            io.state().persisted.last(),
            Some(&(WindowMode::Companion, true))
        );
        let data = controller.lock_data().unwrap();
        assert_eq!(data.actual_mode, None);
        assert!(data.degraded);
        assert!(data.visibility_degraded);
        assert!(!data.runtime_synchronized);
        assert!(data.companion_snapshot.is_some());
    }

    #[test]
    fn explorer_recovery_preserves_manual_hidden_intent() {
        let io = Arc::new(FakeIo::default());
        let controller = controller(io.clone());
        let hide_ack = auto_ack(controller.clone(), io.clone());
        controller.set_user_visible(false).unwrap();
        hide_ack.join().unwrap();
        let desktop_ack = auto_ack(controller.clone(), io.clone());
        controller
            .set_mode(
                "hidden-desktop-before-explorer-loss".into(),
                WindowMode::Desktop,
            )
            .unwrap();
        desktop_ack.join().unwrap();
        {
            let mut state = io.state();
            state.host_alive_results.push_back(Ok(false));
            state.visible.clear();
            state.events.clear();
        }

        controller.check_desktop_host().unwrap();

        let snapshot = controller.snapshot().unwrap();
        assert_eq!(snapshot.actual_mode, Some(WindowMode::Desktop));
        assert!(!snapshot.user_visible);
        assert!(io.state().visible.is_empty());
    }

    #[test]
    fn health_check_is_disabled_unless_actual_mode_is_desktop() {
        let io = Arc::new(FakeIo::default());
        io.state().host_alive_results.push_back(Ok(false));
        let controller = controller(io.clone());

        controller.check_desktop_host().unwrap();

        assert_eq!(io.state().host_alive_results.len(), 1);
        assert!(io.state().events.is_empty());
        assert_eq!(DESKTOP_HOST_HEALTH_INTERVAL, Duration::from_secs(2));
    }

    #[test]
    fn manual_mode_request_cancels_and_joins_an_old_recovery_before_committing() {
        let io = Arc::new(FakeIo::default());
        let started = Arc::new((Mutex::new(false), Condvar::new()));
        let controller = Arc::new(WindowModeController::with_timeout_and_recovery_wait(
            io.clone(),
            true,
            Duration::from_millis(300),
            Arc::new(BlockingRecoveryWait {
                started: started.clone(),
            }),
        ));
        let initial_ack = auto_ack(controller.clone(), io.clone());
        controller
            .set_mode(
                "desktop-before-cancelled-recovery".into(),
                WindowMode::Desktop,
            )
            .unwrap();
        initial_ack.join().unwrap();
        {
            let mut state = io.state();
            state.host_alive_results.push_back(Ok(false));
            state
                .attach_results
                .push_back(Err("Explorer still starting".into()));
            state.events.clear();
        }
        let recovery_ack = auto_ack(controller.clone(), io.clone());
        let recovery = {
            let controller = controller.clone();
            thread::spawn(move || controller.check_desktop_host())
        };
        let (lock, changed) = &*started;
        let mut reached_wait = lock.lock().unwrap();
        while !*reached_wait {
            reached_wait = changed.wait(reached_wait).unwrap();
        }
        drop(reached_wait);

        let companion = controller
            .set_mode(
                "manual-companion-during-recovery".into(),
                WindowMode::Companion,
            )
            .unwrap();
        recovery.join().unwrap().unwrap();
        recovery_ack.join().unwrap();

        assert_eq!(companion.desired_mode, WindowMode::Companion);
        assert_eq!(companion.actual_mode, Some(WindowMode::Companion));
        assert_eq!(
            io.state().persisted.last(),
            Some(&(WindowMode::Companion, true))
        );
        assert!(controller.lock_data().unwrap().recovery.is_none());
    }

    #[test]
    fn manual_cancel_restore_failure_stops_the_new_mode_transaction_and_preserves_snapshot() {
        let (controller, io, join) = cancel_failure_harness(|state| {
            state.restore_error = Some("manual cancel restore failed".into());
        });

        let error = controller
            .set_mode(
                "manual-companion-after-restore-failure".into(),
                WindowMode::Companion,
            )
            .unwrap_err();
        join.join().unwrap();

        assert!(error.contains("manual cancel restore failed"));
        let data = controller.lock_data().unwrap();
        assert_eq!(data.actual_mode, None);
        assert!(data.degraded);
        assert!(data.companion_snapshot.is_some());
        assert_eq!(io.state().attaches, 2);
    }

    #[test]
    fn manual_cancel_visibility_failure_stops_the_new_mode_transaction_and_preserves_snapshot() {
        let (controller, _, join) = cancel_failure_harness(|state| {
            state.visible_results.push_back(Ok(()));
            state
                .visible_results
                .push_back(Err("manual cancel show failed".into()));
            state
                .visible_results
                .push_back(Err("manual cancel hide failed".into()));
        });

        let error = controller
            .set_mode(
                "manual-companion-after-visible-failure".into(),
                WindowMode::Companion,
            )
            .unwrap_err();
        join.join().unwrap();

        assert!(error.contains("manual cancel show failed"));
        let data = controller.lock_data().unwrap();
        assert_eq!(data.actual_mode, None);
        assert!(data.visibility_degraded);
        assert!(!data.runtime_synchronized);
        assert!(data.companion_snapshot.is_some());
    }

    #[test]
    fn manual_cancel_runtime_failure_stops_the_new_mode_transaction_and_preserves_snapshot() {
        let (controller, _, join) = cancel_failure_harness(|state| {
            state.emit_results.push_back(Ok(()));
            state.emit_results.push_back(Ok(()));
            state
                .emit_results
                .push_back(Err("manual cancel pause failed".into()));
        });

        let error = controller
            .set_mode(
                "manual-companion-after-runtime-failure".into(),
                WindowMode::Companion,
            )
            .unwrap_err();
        join.join().unwrap();

        assert!(error.contains("manual cancel pause failed"));
        let data = controller.lock_data().unwrap();
        assert_eq!(data.actual_mode, None);
        assert!(data.visibility_degraded);
        assert!(!data.runtime_synchronized);
        assert!(data.companion_snapshot.is_some());
    }

    #[test]
    fn manual_cancel_persist_failure_stops_the_new_mode_transaction_and_preserves_snapshot() {
        let (controller, io, join) = cancel_failure_harness(|state| {
            state.persist_error = Some("manual cancel persist failed".into());
        });

        let error = controller
            .set_mode(
                "manual-companion-after-persist-failure".into(),
                WindowMode::Companion,
            )
            .unwrap_err();
        join.join().unwrap();

        assert!(error.contains("manual cancel persist failed"));
        let data = controller.lock_data().unwrap();
        assert_eq!(data.actual_mode, None);
        assert!(data.degraded);
        assert!(data.companion_snapshot.is_some());
        assert_eq!(io.state().physical_visible, Some(false));
    }

    #[test]
    fn recovery_generation_failure_rejects_all_waiters_and_later_mode_requests_with_the_same_error()
    {
        let io = Arc::new(FakeIo::default());
        let started = Arc::new((Mutex::new(false), Condvar::new()));
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let controller = Arc::new(WindowModeController::with_timeout_and_recovery_wait(
            io.clone(),
            true,
            Duration::from_millis(300),
            Arc::new(CoordinatedRecoveryWait {
                started: started.clone(),
                release: release.clone(),
            }),
        ));
        let initial_ack = auto_ack(controller.clone(), io.clone());
        controller
            .set_mode("desktop-before-generation-gate".into(), WindowMode::Desktop)
            .unwrap();
        initial_ack.join().unwrap();
        {
            let mut state = io.state();
            state.host_alive_results.push_back(Ok(false));
            state
                .attach_results
                .push_back(Err("Explorer still starting".into()));
            state.restore_error = Some("shared generation restore failed".into());
            state.captures = 0;
            state.attaches = 0;
            state.persisted.clear();
            state.events.clear();
        }
        let recovery_ack = auto_ack(controller.clone(), io.clone());
        let recovery = {
            let controller = controller.clone();
            thread::spawn(move || controller.check_desktop_host())
        };
        let (lock, changed) = &*started;
        let mut reached_wait = lock.lock().unwrap();
        while !*reached_wait {
            reached_wait = changed.wait(reached_wait).unwrap();
        }
        drop(reached_wait);
        let companion_waiter = {
            let controller = controller.clone();
            thread::spawn(move || {
                controller.set_mode("generation-waiter-companion".into(), WindowMode::Companion)
            })
        };
        let desktop_waiter = {
            let controller = controller.clone();
            thread::spawn(move || {
                controller.set_mode("generation-waiter-desktop".into(), WindowMode::Desktop)
            })
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        while controller.lock_data().unwrap().recovery_waiters < 2 {
            assert!(
                Instant::now() < deadline,
                "both waiters did not join the recovery generation"
            );
            thread::yield_now();
        }
        let (lock, changed) = &*release;
        *lock.lock().unwrap() = true;
        changed.notify_all();

        let recovery_error = recovery.join().unwrap().unwrap_err();
        recovery_ack.join().unwrap();
        let companion_error = companion_waiter.join().unwrap().unwrap_err();
        let desktop_error = desktop_waiter.join().unwrap().unwrap_err();
        assert_eq!(companion_error, recovery_error);
        assert_eq!(desktop_error, recovery_error);
        let effects = {
            let state = io.state();
            (state.captures, state.attaches, state.persisted.len())
        };

        let later_error = controller
            .set_mode("generation-later-request".into(), WindowMode::Companion)
            .unwrap_err();
        assert_eq!(later_error, recovery_error);
        let state = io.state();
        assert_eq!(
            (state.captures, state.attaches, state.persisted.len()),
            effects
        );
        assert_eq!(state.captures, 0);
        assert_eq!(state.attaches, 1);
        assert_eq!(state.persisted.len(), 1);
    }

    #[test]
    fn successful_recovery_cancellation_releases_all_waiters_without_deadlock_or_failure_gate() {
        let io = Arc::new(FakeIo::default());
        let started = Arc::new((Mutex::new(false), Condvar::new()));
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let controller = Arc::new(WindowModeController::with_timeout_and_recovery_wait(
            io.clone(),
            true,
            Duration::from_millis(300),
            Arc::new(CoordinatedRecoveryWait {
                started: started.clone(),
                release: release.clone(),
            }),
        ));
        let initial_ack = auto_ack(controller.clone(), io.clone());
        controller
            .set_mode(
                "desktop-before-successful-generation-cancel".into(),
                WindowMode::Desktop,
            )
            .unwrap();
        initial_ack.join().unwrap();
        {
            let mut state = io.state();
            state.host_alive_results.push_back(Ok(false));
            state
                .attach_results
                .push_back(Err("Explorer still starting".into()));
            state.events.clear();
        }
        let recovery_ack = auto_ack(controller.clone(), io.clone());
        let recovery = {
            let controller = controller.clone();
            thread::spawn(move || controller.check_desktop_host())
        };
        let (lock, changed) = &*started;
        let mut reached_wait = lock.lock().unwrap();
        while !*reached_wait {
            reached_wait = changed.wait(reached_wait).unwrap();
        }
        drop(reached_wait);
        let first = {
            let controller = controller.clone();
            thread::spawn(move || {
                controller.set_mode("successful-waiter-one".into(), WindowMode::Companion)
            })
        };
        let second = {
            let controller = controller.clone();
            thread::spawn(move || {
                controller.set_mode("successful-waiter-two".into(), WindowMode::Companion)
            })
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        while controller.lock_data().unwrap().recovery_waiters < 2 {
            assert!(
                Instant::now() < deadline,
                "both successful waiters did not join"
            );
            thread::yield_now();
        }
        let (lock, changed) = &*release;
        *lock.lock().unwrap() = true;
        changed.notify_all();

        assert_eq!(
            recovery.join().unwrap().unwrap().actual_mode,
            Some(WindowMode::Companion)
        );
        recovery_ack.join().unwrap();
        assert_eq!(
            first.join().unwrap().unwrap().actual_mode,
            Some(WindowMode::Companion)
        );
        assert_eq!(
            second.join().unwrap().unwrap().actual_mode,
            Some(WindowMode::Companion)
        );
        let data = controller.lock_data().unwrap();
        assert!(data.recovery_failure_gate.is_none());
        assert_eq!(data.recovery_waiters, 0);
    }

    #[test]
    fn shutdown_restores_desktop_snapshot_and_reports_restore_failure_without_aborting() {
        let io = Arc::new(FakeIo::default());
        let controller = controller(io.clone());
        let initial_ack = auto_ack(controller.clone(), io.clone());
        controller
            .set_mode("desktop-before-shutdown".into(), WindowMode::Desktop)
            .unwrap();
        initial_ack.join().unwrap();
        io.state().restore_error = Some("restore failed during shutdown".into());

        controller.shutdown();
        controller.shutdown();

        assert_eq!(io.state().restores, 1);
        assert!(io
            .state()
            .recovery_notices
            .iter()
            .any(|message| message.contains("restore failed during shutdown")));
    }

    #[test]
    fn shutdown_cancels_recovery_and_restores_without_rewriting_saved_desktop_intent() {
        let io = Arc::new(FakeIo::default());
        let started = Arc::new((Mutex::new(false), Condvar::new()));
        let controller = Arc::new(WindowModeController::with_timeout_and_recovery_wait(
            io.clone(),
            true,
            Duration::from_millis(300),
            Arc::new(BlockingRecoveryWait {
                started: started.clone(),
            }),
        ));
        let initial_ack = auto_ack(controller.clone(), io.clone());
        controller
            .set_mode(
                "desktop-before-recovery-shutdown".into(),
                WindowMode::Desktop,
            )
            .unwrap();
        initial_ack.join().unwrap();
        {
            let mut state = io.state();
            state.host_alive_results.push_back(Ok(false));
            state
                .attach_results
                .push_back(Err("Explorer still restarting".into()));
            state.events.clear();
        }
        let recovery_ack = auto_ack(controller.clone(), io.clone());
        let recovery = {
            let controller = controller.clone();
            thread::spawn(move || controller.check_desktop_host())
        };
        let (lock, changed) = &*started;
        let mut reached_wait = lock.lock().unwrap();
        while !*reached_wait {
            reached_wait = changed.wait(reached_wait).unwrap();
        }
        drop(reached_wait);

        controller.shutdown();
        recovery.join().unwrap().unwrap();
        recovery_ack.join().unwrap();

        assert_eq!(io.state().restores, 1);
        assert_eq!(
            io.state().persisted.last(),
            Some(&(WindowMode::Desktop, true))
        );
        assert!(controller.lock_data().unwrap().shutdown_complete);
        assert!(controller.lock_data().unwrap().recovery.is_none());
    }
}
