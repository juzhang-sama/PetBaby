use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum WindowMode {
    Companion,
    Desktop,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DesktopHostState {
    Detached,
    WorkerW { parent: isize },
    BottomFallback,
    Lost,
}

impl DesktopHostState {
    fn is_healthy(self) -> bool {
        matches!(
            self,
            DesktopHostState::WorkerW { parent } if parent != 0
        ) || self == DesktopHostState::BottomFallback
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DesktopHostAttempt {
    WorkerW,
    BottomFallback,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum SuppressionReason {
    Fullscreen,
    LockSleep,
    VirtualDesktopMismatch,
    ExplorerLost,
    Transition,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum VirtualDesktopStatus {
    Current,
    Mismatch,
    Unsupported,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowModeEvent {
    RequestMode(WindowMode),
    HostAttached(DesktopHostState),
    HostFailed(DesktopHostAttempt),
    ExplorerLost,
    ExplorerRecovered,
    FullscreenChanged(bool),
    LockSleepChanged(bool),
    VirtualDesktopChanged(VirtualDesktopStatus),
    UserVisibilityChanged(bool),
    TransitionFinished(WindowMode),
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowModeAction {
    Show,
    Hide,
    TryDesktopHost(DesktopHostAttempt),
    RestoreCompanionHost,
    ReportVirtualDesktopUnsupported,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowModeState {
    desired_mode: WindowMode,
    user_visible: bool,
    suppressions: BTreeSet<SuppressionReason>,
    desktop_host: DesktopHostState,
    transition: Option<WindowMode>,
    fullscreen_active: bool,
    desktop_attempt: Option<DesktopHostAttempt>,
}

#[allow(dead_code)]
impl WindowModeState {
    pub fn new() -> Self {
        Self {
            desired_mode: WindowMode::Companion,
            user_visible: true,
            suppressions: BTreeSet::new(),
            desktop_host: DesktopHostState::Detached,
            transition: None,
            fullscreen_active: false,
            desktop_attempt: None,
        }
    }

    pub fn desired_mode(&self) -> WindowMode {
        self.desired_mode
    }

    pub fn user_visible(&self) -> bool {
        self.user_visible
    }

    pub fn suppressions(&self) -> &BTreeSet<SuppressionReason> {
        &self.suppressions
    }

    pub fn desktop_host(&self) -> DesktopHostState {
        self.desktop_host
    }

    pub fn transition(&self) -> Option<WindowMode> {
        self.transition
    }

    pub fn visibility_action(&self) -> WindowModeAction {
        let common_suppression = self.suppressions.iter().any(|reason| {
            matches!(
                reason,
                SuppressionReason::LockSleep
                    | SuppressionReason::VirtualDesktopMismatch
                    | SuppressionReason::Transition
            )
        });
        let mode_suppression = match self.desired_mode {
            WindowMode::Companion => self.suppressions.contains(&SuppressionReason::Fullscreen),
            WindowMode::Desktop => {
                self.suppressions.contains(&SuppressionReason::ExplorerLost)
                    || !self.desktop_host.is_healthy()
            }
        };

        if self.user_visible && !common_suppression && !mode_suppression {
            WindowModeAction::Show
        } else {
            WindowModeAction::Hide
        }
    }

    fn refresh_mode_specific_suppressions(&mut self) {
        match self.desired_mode {
            WindowMode::Companion => {
                if self.fullscreen_active {
                    self.suppressions.insert(SuppressionReason::Fullscreen);
                } else {
                    self.suppressions.remove(&SuppressionReason::Fullscreen);
                }
                self.suppressions.remove(&SuppressionReason::ExplorerLost);
            }
            WindowMode::Desktop => {
                self.suppressions.remove(&SuppressionReason::Fullscreen);
                if self.desktop_host == DesktopHostState::Lost {
                    self.suppressions.insert(SuppressionReason::ExplorerLost);
                }
            }
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowModeReduction {
    pub state: WindowModeState,
    pub actions: Vec<WindowModeAction>,
}

#[allow(dead_code)]
pub fn reduce_window_mode(
    mut state: WindowModeState,
    event: WindowModeEvent,
) -> WindowModeReduction {
    let visibility_before = state.visibility_action();
    let mut actions = Vec::new();
    let mut append_visibility_change = false;

    match event {
        WindowModeEvent::RequestMode(requested) => {
            if requested == state.desired_mode {
                return WindowModeReduction { state, actions };
            }
            if state.transition.is_some() {
                return WindowModeReduction { state, actions };
            }

            state.desired_mode = requested;
            state.transition = Some(requested);
            state.suppressions.insert(SuppressionReason::Transition);
            state.refresh_mode_specific_suppressions();
            actions.push(WindowModeAction::Hide);
            match requested {
                WindowMode::Companion => {
                    state.desktop_attempt = None;
                    actions.push(WindowModeAction::RestoreCompanionHost);
                }
                WindowMode::Desktop => {
                    if state.desktop_host != DesktopHostState::Lost {
                        state.desktop_host = DesktopHostState::Detached;
                        state.desktop_attempt = Some(DesktopHostAttempt::WorkerW);
                        actions.push(WindowModeAction::TryDesktopHost(
                            DesktopHostAttempt::WorkerW,
                        ));
                    }
                }
            }
        }
        WindowModeEvent::HostAttached(DesktopHostState::WorkerW { parent: 0 })
            if state.desired_mode == WindowMode::Desktop
                && state.transition == Some(WindowMode::Desktop)
                && state.desktop_attempt == Some(DesktopHostAttempt::WorkerW) =>
        {
            state.desktop_host = DesktopHostState::Detached;
            state.desktop_attempt = Some(DesktopHostAttempt::BottomFallback);
            actions.push(WindowModeAction::TryDesktopHost(
                DesktopHostAttempt::BottomFallback,
            ));
        }
        WindowModeEvent::HostAttached(host)
            if state.desired_mode == WindowMode::Desktop
                && state.transition == Some(WindowMode::Desktop)
                && host_matches_attempt(host, state.desktop_attempt)
                && host.is_healthy() =>
        {
            state.desktop_host = host;
            state.desktop_attempt = None;
            state.suppressions.remove(&SuppressionReason::ExplorerLost);
        }
        WindowModeEvent::HostFailed(DesktopHostAttempt::WorkerW)
            if state.desired_mode == WindowMode::Desktop
                && state.transition == Some(WindowMode::Desktop)
                && state.desktop_attempt == Some(DesktopHostAttempt::WorkerW) =>
        {
            state.desktop_host = DesktopHostState::Detached;
            state.desktop_attempt = Some(DesktopHostAttempt::BottomFallback);
            actions.push(WindowModeAction::TryDesktopHost(
                DesktopHostAttempt::BottomFallback,
            ));
        }
        WindowModeEvent::HostFailed(DesktopHostAttempt::BottomFallback)
            if state.desired_mode == WindowMode::Desktop
                && state.transition == Some(WindowMode::Desktop)
                && state.desktop_attempt == Some(DesktopHostAttempt::BottomFallback) =>
        {
            state.desired_mode = WindowMode::Companion;
            state.transition = Some(WindowMode::Companion);
            state.desktop_host = DesktopHostState::Detached;
            state.desktop_attempt = None;
            state.suppressions.remove(&SuppressionReason::ExplorerLost);
            state.refresh_mode_specific_suppressions();
            actions.push(WindowModeAction::RestoreCompanionHost);
        }
        WindowModeEvent::ExplorerLost => {
            state.desktop_host = DesktopHostState::Lost;
            state.desktop_attempt = None;
            if state.desired_mode == WindowMode::Desktop {
                state.suppressions.insert(SuppressionReason::ExplorerLost);
                append_visibility_change = true;
            }
        }
        WindowModeEvent::ExplorerRecovered => {
            if state.desired_mode == WindowMode::Desktop
                && state.desktop_host == DesktopHostState::Lost
                && matches!(state.transition, None | Some(WindowMode::Desktop))
                && state.desktop_attempt.is_none()
            {
                if state.transition.is_none() {
                    state.transition = Some(WindowMode::Desktop);
                    state.suppressions.insert(SuppressionReason::Transition);
                }
                state.desktop_attempt = Some(DesktopHostAttempt::WorkerW);
                actions.push(WindowModeAction::TryDesktopHost(
                    DesktopHostAttempt::WorkerW,
                ));
            } else if state.desired_mode == WindowMode::Companion
                && state.desktop_host == DesktopHostState::Lost
            {
                state.desktop_host = DesktopHostState::Detached;
            }
        }
        WindowModeEvent::FullscreenChanged(active) => {
            state.fullscreen_active = active;
            state.refresh_mode_specific_suppressions();
            append_visibility_change = true;
        }
        WindowModeEvent::LockSleepChanged(active) => {
            set_suppression(&mut state, SuppressionReason::LockSleep, active);
            append_visibility_change = true;
        }
        WindowModeEvent::VirtualDesktopChanged(VirtualDesktopStatus::Current) => {
            state
                .suppressions
                .remove(&SuppressionReason::VirtualDesktopMismatch);
            append_visibility_change = true;
        }
        WindowModeEvent::VirtualDesktopChanged(VirtualDesktopStatus::Mismatch) => {
            state
                .suppressions
                .insert(SuppressionReason::VirtualDesktopMismatch);
            append_visibility_change = true;
        }
        WindowModeEvent::VirtualDesktopChanged(VirtualDesktopStatus::Unsupported) => {
            actions.push(WindowModeAction::ReportVirtualDesktopUnsupported);
        }
        WindowModeEvent::UserVisibilityChanged(visible) => {
            state.user_visible = visible;
            append_visibility_change = true;
        }
        WindowModeEvent::TransitionFinished(applied)
            if state.transition == Some(applied)
                && state.desired_mode == applied
                && (applied == WindowMode::Companion || state.desktop_host.is_healthy()) =>
        {
            if applied == WindowMode::Companion {
                state.desktop_host = DesktopHostState::Detached;
            }
            state.desktop_attempt = None;
            state.transition = None;
            state.suppressions.remove(&SuppressionReason::Transition);
            actions.push(state.visibility_action());
        }
        WindowModeEvent::HostAttached(_)
        | WindowModeEvent::HostFailed(_)
        | WindowModeEvent::TransitionFinished(_) => {}
    }

    if append_visibility_change {
        let visibility_after = state.visibility_action();
        if visibility_after != visibility_before {
            actions.push(visibility_after);
        }
    }

    WindowModeReduction { state, actions }
}

fn host_matches_attempt(host: DesktopHostState, attempt: Option<DesktopHostAttempt>) -> bool {
    matches!(
        (host, attempt),
        (
            DesktopHostState::WorkerW { .. },
            Some(DesktopHostAttempt::WorkerW)
        ) | (
            DesktopHostState::BottomFallback,
            Some(DesktopHostAttempt::BottomFallback)
        )
    )
}

fn set_suppression(state: &mut WindowModeState, reason: SuppressionReason, active: bool) {
    if active {
        state.suppressions.insert(reason);
    } else {
        state.suppressions.remove(&reason);
    }
}

#[cfg(test)]
mod mode_tests {
    use super::*;

    fn take_state(
        reduction: WindowModeReduction,
        expected_actions: &[WindowModeAction],
    ) -> WindowModeState {
        assert_eq!(reduction.actions, expected_actions);
        reduction.state
    }

    fn apply_event(
        state: WindowModeState,
        event: WindowModeEvent,
        expected_actions: &[WindowModeAction],
    ) -> WindowModeState {
        take_state(reduce_window_mode(state, event), expected_actions)
    }

    fn requested_desktop_state() -> WindowModeState {
        take_state(
            reduce_window_mode(
                WindowModeState::new(),
                WindowModeEvent::RequestMode(WindowMode::Desktop),
            ),
            &[
                WindowModeAction::Hide,
                WindowModeAction::TryDesktopHost(DesktopHostAttempt::WorkerW),
            ],
        )
    }

    fn state(mode: WindowMode) -> WindowModeState {
        let companion = WindowModeState::new();
        if mode == WindowMode::Companion {
            return companion;
        }

        let attached = take_state(
            reduce_window_mode(
                requested_desktop_state(),
                WindowModeEvent::HostAttached(DesktopHostState::WorkerW { parent: 42 }),
            ),
            &[],
        );
        take_state(
            reduce_window_mode(
                attached,
                WindowModeEvent::TransitionFinished(WindowMode::Desktop),
            ),
            &[WindowModeAction::Show],
        )
    }

    #[test]
    fn construction_starts_only_from_a_legal_companion_state() {
        let state = WindowModeState::new();

        assert_eq!(state.desired_mode, WindowMode::Companion);
        assert_eq!(state.desktop_host, DesktopHostState::Detached);
        assert_eq!(state.transition, None);
        assert_eq!(state.visibility_action(), WindowModeAction::Show);
    }

    #[test]
    fn preserves_companion_json_and_rejects_future_unknown_modes() {
        assert_eq!(
            serde_json::from_str::<WindowMode>("\"companion\"").unwrap(),
            WindowMode::Companion
        );
        assert_eq!(
            serde_json::from_str::<WindowMode>("\"desktop\"").unwrap(),
            WindowMode::Desktop
        );
        assert!(serde_json::from_str::<WindowMode>("\"floating\"").is_err());
    }

    #[test]
    fn fullscreen_suppresses_companion_but_not_desktop() {
        let companion = take_state(
            reduce_window_mode(
                state(WindowMode::Companion),
                WindowModeEvent::FullscreenChanged(true),
            ),
            &[WindowModeAction::Hide],
        );
        assert!(companion
            .suppressions
            .contains(&SuppressionReason::Fullscreen));
        assert_eq!(companion.visibility_action(), WindowModeAction::Hide);

        let desktop = take_state(
            reduce_window_mode(
                state(WindowMode::Desktop),
                WindowModeEvent::FullscreenChanged(true),
            ),
            &[],
        );
        assert!(!desktop
            .suppressions
            .contains(&SuppressionReason::Fullscreen));
        assert_eq!(desktop.visibility_action(), WindowModeAction::Show);
    }

    #[test]
    fn clearing_fullscreen_does_not_override_manual_hide() {
        let manually_hidden = take_state(
            reduce_window_mode(
                state(WindowMode::Companion),
                WindowModeEvent::UserVisibilityChanged(false),
            ),
            &[WindowModeAction::Hide],
        );
        let fullscreen = take_state(
            reduce_window_mode(manually_hidden, WindowModeEvent::FullscreenChanged(true)),
            &[],
        );
        let state = take_state(
            reduce_window_mode(fullscreen, WindowModeEvent::FullscreenChanged(false)),
            &[],
        );
        assert_eq!(state.visibility_action(), WindowModeAction::Hide);
    }

    #[test]
    fn companion_to_desktop_requests_workerw_while_hidden() {
        let reduction = reduce_window_mode(
            state(WindowMode::Companion),
            WindowModeEvent::RequestMode(WindowMode::Desktop),
        );

        assert_eq!(reduction.state.desired_mode, WindowMode::Desktop);
        assert_eq!(reduction.state.transition, Some(WindowMode::Desktop));
        assert!(reduction
            .state
            .suppressions
            .contains(&SuppressionReason::Transition));
        assert_eq!(
            reduction.actions,
            vec![
                WindowModeAction::Hide,
                WindowModeAction::TryDesktopHost(DesktopHostAttempt::WorkerW),
            ]
        );
    }

    #[test]
    fn workerw_success_finishes_desktop_transition() {
        let started = requested_desktop_state();
        let attached = take_state(
            reduce_window_mode(
                started,
                WindowModeEvent::HostAttached(DesktopHostState::WorkerW { parent: 42 }),
            ),
            &[],
        );
        let reduction = reduce_window_mode(
            attached,
            WindowModeEvent::TransitionFinished(WindowMode::Desktop),
        );

        assert_eq!(
            reduction.state.desktop_host,
            DesktopHostState::WorkerW { parent: 42 }
        );
        assert_eq!(reduction.state.transition, None);
        assert_eq!(reduction.actions, vec![WindowModeAction::Show]);
    }

    #[test]
    fn workerw_failure_tries_bottom_fallback() {
        let started = requested_desktop_state();
        let reduction = reduce_window_mode(
            started,
            WindowModeEvent::HostFailed(DesktopHostAttempt::WorkerW),
        );

        assert_eq!(reduction.state.transition, Some(WindowMode::Desktop));
        assert_eq!(reduction.state.desktop_host, DesktopHostState::Detached);
        assert_eq!(
            reduction.actions,
            vec![WindowModeAction::TryDesktopHost(
                DesktopHostAttempt::BottomFallback,
            )]
        );
    }

    #[test]
    fn zero_workerw_parent_fails_closed_and_advances_to_bottom_fallback() {
        let invalid_attach = reduce_window_mode(
            requested_desktop_state(),
            WindowModeEvent::HostAttached(DesktopHostState::WorkerW { parent: 0 }),
        );

        assert_eq!(
            invalid_attach.state.desktop_host,
            DesktopHostState::Detached
        );
        assert_eq!(
            invalid_attach.actions,
            vec![WindowModeAction::TryDesktopHost(
                DesktopHostAttempt::BottomFallback,
            )]
        );

        let duplicate = reduce_window_mode(
            invalid_attach.state,
            WindowModeEvent::HostAttached(DesktopHostState::WorkerW { parent: 0 }),
        );
        assert!(duplicate.actions.is_empty());
        assert_eq!(duplicate.state.transition, Some(WindowMode::Desktop));
        assert_eq!(duplicate.state.visibility_action(), WindowModeAction::Hide);
    }

    #[test]
    fn repeated_workerw_failure_is_idempotent() {
        let started = requested_desktop_state();
        let first = reduce_window_mode(
            started,
            WindowModeEvent::HostFailed(DesktopHostAttempt::WorkerW),
        );
        let duplicate = reduce_window_mode(
            first.state.clone(),
            WindowModeEvent::HostFailed(DesktopHostAttempt::WorkerW),
        );

        assert_eq!(duplicate.state, first.state);
        assert!(duplicate.actions.is_empty());
    }

    #[test]
    fn out_of_order_bottom_attach_cannot_finish_desktop_transition() {
        let started = requested_desktop_state();
        let out_of_order = reduce_window_mode(
            started.clone(),
            WindowModeEvent::HostAttached(DesktopHostState::BottomFallback),
        );
        assert_eq!(out_of_order.state, started);
        assert!(out_of_order.actions.is_empty());

        let finish = reduce_window_mode(
            out_of_order.state,
            WindowModeEvent::TransitionFinished(WindowMode::Desktop),
        );
        assert_eq!(finish.state.transition, Some(WindowMode::Desktop));
        assert_eq!(finish.state.visibility_action(), WindowModeAction::Hide);
    }

    #[test]
    fn bottom_fallback_success_can_commit_desktop() {
        let bottom_attempt = take_state(
            reduce_window_mode(
                requested_desktop_state(),
                WindowModeEvent::HostFailed(DesktopHostAttempt::WorkerW),
            ),
            &[WindowModeAction::TryDesktopHost(
                DesktopHostAttempt::BottomFallback,
            )],
        );
        let attached = take_state(
            reduce_window_mode(
                bottom_attempt,
                WindowModeEvent::HostAttached(DesktopHostState::BottomFallback),
            ),
            &[],
        );
        let finished = take_state(
            reduce_window_mode(
                attached,
                WindowModeEvent::TransitionFinished(WindowMode::Desktop),
            ),
            &[WindowModeAction::Show],
        );

        assert_eq!(finished.desired_mode, WindowMode::Desktop);
        assert_eq!(finished.desktop_host, DesktopHostState::BottomFallback);
        assert_eq!(finished.transition, None);
        assert_eq!(finished.visibility_action(), WindowModeAction::Show);
    }

    #[test]
    fn double_host_failure_rolls_back_to_companion() {
        let started = take_state(
            reduce_window_mode(
                requested_desktop_state(),
                WindowModeEvent::HostFailed(DesktopHostAttempt::WorkerW),
            ),
            &[WindowModeAction::TryDesktopHost(
                DesktopHostAttempt::BottomFallback,
            )],
        );
        let rollback = reduce_window_mode(
            started,
            WindowModeEvent::HostFailed(DesktopHostAttempt::BottomFallback),
        );

        assert_eq!(rollback.state.desired_mode, WindowMode::Companion);
        assert_eq!(rollback.state.transition, Some(WindowMode::Companion));
        assert_eq!(rollback.state.desktop_host, DesktopHostState::Detached);
        assert_eq!(
            rollback.actions,
            vec![WindowModeAction::RestoreCompanionHost]
        );
        assert_eq!(rollback.state.visibility_action(), WindowModeAction::Hide);

        let finished = reduce_window_mode(
            rollback.state,
            WindowModeEvent::TransitionFinished(WindowMode::Companion),
        );
        assert_eq!(finished.state.transition, None);
        assert_eq!(finished.actions, vec![WindowModeAction::Show]);
    }

    #[test]
    fn desktop_to_companion_restores_host_before_finishing() {
        let reduction = reduce_window_mode(
            state(WindowMode::Desktop),
            WindowModeEvent::RequestMode(WindowMode::Companion),
        );

        assert_eq!(reduction.state.desired_mode, WindowMode::Companion);
        assert_eq!(reduction.state.transition, Some(WindowMode::Companion));
        assert_eq!(
            reduction.actions,
            vec![
                WindowModeAction::Hide,
                WindowModeAction::RestoreCompanionHost,
            ]
        );

        let finished = take_state(
            reduce_window_mode(
                reduction.state,
                WindowModeEvent::TransitionFinished(WindowMode::Companion),
            ),
            &[WindowModeAction::Show],
        );
        assert_eq!(finished.desktop_host, DesktopHostState::Detached);
        assert_eq!(finished.transition, None);
        assert_eq!(finished.visibility_action(), WindowModeAction::Show);
    }

    #[test]
    fn repeated_mode_request_is_idempotent() {
        let initial = state(WindowMode::Companion);
        let same = reduce_window_mode(
            initial.clone(),
            WindowModeEvent::RequestMode(WindowMode::Companion),
        );
        assert_eq!(same.state, initial);
        assert!(same.actions.is_empty());

        let transitioning = take_state(
            reduce_window_mode(initial, WindowModeEvent::RequestMode(WindowMode::Desktop)),
            &[
                WindowModeAction::Hide,
                WindowModeAction::TryDesktopHost(DesktopHostAttempt::WorkerW),
            ],
        );
        let duplicate = reduce_window_mode(
            transitioning.clone(),
            WindowModeEvent::RequestMode(WindowMode::Desktop),
        );
        assert_eq!(duplicate.state, transitioning);
        assert!(duplicate.actions.is_empty());
    }

    #[test]
    fn explorer_loss_hides_desktop_until_a_new_host_is_attached() {
        let fullscreen = take_state(
            reduce_window_mode(
                state(WindowMode::Desktop),
                WindowModeEvent::FullscreenChanged(true),
            ),
            &[],
        );
        let lost = take_state(
            reduce_window_mode(fullscreen, WindowModeEvent::ExplorerLost),
            &[WindowModeAction::Hide],
        );
        assert_eq!(lost.desktop_host, DesktopHostState::Lost);
        assert!(lost.suppressions.contains(&SuppressionReason::ExplorerLost));
        assert_eq!(lost.visibility_action(), WindowModeAction::Hide);

        let still_lost = take_state(
            reduce_window_mode(lost, WindowModeEvent::FullscreenChanged(false)),
            &[],
        );
        assert_eq!(still_lost.visibility_action(), WindowModeAction::Hide);

        let recovered = reduce_window_mode(still_lost, WindowModeEvent::ExplorerRecovered);
        assert!(recovered
            .state
            .suppressions
            .contains(&SuppressionReason::ExplorerLost));
        assert_eq!(recovered.state.visibility_action(), WindowModeAction::Hide);
        assert_eq!(
            recovered.actions,
            vec![WindowModeAction::TryDesktopHost(
                DesktopHostAttempt::WorkerW,
            )]
        );
    }

    #[test]
    fn explorer_recovery_resumes_a_desktop_request_started_while_lost() {
        let lost_companion = take_state(
            reduce_window_mode(state(WindowMode::Companion), WindowModeEvent::ExplorerLost),
            &[],
        );
        assert_eq!(lost_companion.visibility_action(), WindowModeAction::Show);

        let requested = reduce_window_mode(
            lost_companion,
            WindowModeEvent::RequestMode(WindowMode::Desktop),
        );
        assert_eq!(requested.actions, vec![WindowModeAction::Hide]);
        assert!(requested
            .state
            .suppressions
            .contains(&SuppressionReason::ExplorerLost));

        let recovered = reduce_window_mode(requested.state, WindowModeEvent::ExplorerRecovered);
        assert_eq!(
            recovered.actions,
            vec![WindowModeAction::TryDesktopHost(
                DesktopHostAttempt::WorkerW,
            )]
        );
        assert_eq!(recovered.state.transition, Some(WindowMode::Desktop));
    }

    #[test]
    fn repeated_explorer_recovery_does_not_restart_an_inflight_host_attempt() {
        let lost = take_state(
            reduce_window_mode(state(WindowMode::Desktop), WindowModeEvent::ExplorerLost),
            &[WindowModeAction::Hide],
        );
        let first = reduce_window_mode(lost, WindowModeEvent::ExplorerRecovered);
        assert_eq!(
            first.actions,
            vec![WindowModeAction::TryDesktopHost(
                DesktopHostAttempt::WorkerW,
            )]
        );

        let duplicate = reduce_window_mode(first.state, WindowModeEvent::ExplorerRecovered);
        assert!(duplicate.actions.is_empty());

        let workerw_failed = reduce_window_mode(
            duplicate.state,
            WindowModeEvent::HostFailed(DesktopHostAttempt::WorkerW),
        );
        assert_eq!(
            workerw_failed.actions,
            vec![WindowModeAction::TryDesktopHost(
                DesktopHostAttempt::BottomFallback,
            )]
        );

        let attached = reduce_window_mode(
            workerw_failed.state,
            WindowModeEvent::HostAttached(DesktopHostState::BottomFallback),
        );
        let finished = reduce_window_mode(
            attached.state,
            WindowModeEvent::TransitionFinished(WindowMode::Desktop),
        );
        assert_eq!(finished.actions, vec![WindowModeAction::Show]);
    }

    #[test]
    fn suppression_clear_order_never_overrides_manual_hide() {
        let mut first_order = apply_event(
            state(WindowMode::Companion),
            WindowModeEvent::UserVisibilityChanged(false),
            &[WindowModeAction::Hide],
        );
        first_order = apply_event(first_order, WindowModeEvent::LockSleepChanged(true), &[]);
        first_order = apply_event(
            first_order,
            WindowModeEvent::VirtualDesktopChanged(VirtualDesktopStatus::Mismatch),
            &[],
        );
        first_order = apply_event(first_order, WindowModeEvent::LockSleepChanged(false), &[]);
        first_order = apply_event(
            first_order,
            WindowModeEvent::VirtualDesktopChanged(VirtualDesktopStatus::Current),
            &[],
        );

        let mut second_order = apply_event(
            state(WindowMode::Companion),
            WindowModeEvent::VirtualDesktopChanged(VirtualDesktopStatus::Mismatch),
            &[WindowModeAction::Hide],
        );
        second_order = apply_event(second_order, WindowModeEvent::LockSleepChanged(true), &[]);
        second_order = apply_event(
            second_order,
            WindowModeEvent::VirtualDesktopChanged(VirtualDesktopStatus::Current),
            &[],
        );
        second_order = apply_event(
            second_order,
            WindowModeEvent::LockSleepChanged(false),
            &[WindowModeAction::Show],
        );
        second_order = apply_event(
            second_order,
            WindowModeEvent::UserVisibilityChanged(false),
            &[WindowModeAction::Hide],
        );

        assert_eq!(first_order.visibility_action(), WindowModeAction::Hide);
        assert_eq!(second_order.visibility_action(), WindowModeAction::Hide);
        assert_eq!(first_order.suppressions, second_order.suppressions);
    }

    #[test]
    fn unsupported_virtual_desktop_is_only_reported() {
        let reduction = reduce_window_mode(
            state(WindowMode::Companion),
            WindowModeEvent::VirtualDesktopChanged(VirtualDesktopStatus::Unsupported),
        );

        assert!(!reduction
            .state
            .suppressions
            .contains(&SuppressionReason::VirtualDesktopMismatch));
        assert_eq!(
            reduction.actions,
            vec![WindowModeAction::ReportVirtualDesktopUnsupported]
        );
        assert_eq!(reduction.state.visibility_action(), WindowModeAction::Show);
    }

    #[test]
    fn mismatched_transition_finish_fails_closed() {
        let started = requested_desktop_state();
        let reduction = reduce_window_mode(
            started.clone(),
            WindowModeEvent::TransitionFinished(WindowMode::Companion),
        );

        assert_eq!(reduction.state, started);
        assert!(reduction.actions.is_empty());
        assert_eq!(reduction.state.visibility_action(), WindowModeAction::Hide);
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RegionSpan {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HitRegionPayload {
    pub canvas_width: i32,
    pub canvas_height: i32,
    pub scale_factor: f64,
    pub spans: Vec<RegionSpan>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HitRegionEvidence {
    pub span_count: usize,
    pub applied: bool,
    pub strategy: &'static str,
    pub scale_factor: f64,
}

pub fn normalize_spans(payload: &HitRegionPayload) -> Result<Vec<RegionSpan>, &'static str> {
    if payload.canvas_width <= 0 || payload.canvas_height <= 0 || payload.scale_factor <= 0.0 {
        return Err("canvas dimensions must be positive");
    }
    let spans = payload
        .spans
        .iter()
        .filter_map(|span| {
            let clipped = RegionSpan {
                left: span.left.clamp(0, payload.canvas_width),
                top: span.top.clamp(0, payload.canvas_height),
                right: span.right.clamp(0, payload.canvas_width),
                bottom: span.bottom.clamp(0, payload.canvas_height),
            };
            (clipped.left < clipped.right && clipped.top < clipped.bottom).then_some(clipped)
        })
        .collect();
    Ok(spans)
}

pub fn scale_spans(spans: &[RegionSpan], scale_factor: f64) -> Vec<RegionSpan> {
    spans
        .iter()
        .map(|span| RegionSpan {
            left: (span.left as f64 * scale_factor).floor() as i32,
            top: (span.top as f64 * scale_factor).floor() as i32,
            right: (span.right as f64 * scale_factor).ceil() as i32,
            bottom: (span.bottom as f64 * scale_factor).ceil() as i32,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_positive_canvas() {
        let payload = HitRegionPayload {
            canvas_width: 0,
            canvas_height: 10,
            scale_factor: 1.0,
            spans: vec![],
        };
        assert_eq!(
            normalize_spans(&payload),
            Err("canvas dimensions must be positive")
        );
    }

    #[test]
    fn clips_spans_and_removes_empty_rows() {
        let payload = HitRegionPayload {
            canvas_width: 100,
            canvas_height: 50,
            scale_factor: 1.0,
            spans: vec![
                RegionSpan {
                    left: -5,
                    top: 2,
                    right: 20,
                    bottom: 4,
                },
                RegionSpan {
                    left: 30,
                    top: 3,
                    right: 30,
                    bottom: 5,
                },
            ],
        };
        assert_eq!(
            normalize_spans(&payload).unwrap(),
            vec![RegionSpan {
                left: 0,
                top: 2,
                right: 20,
                bottom: 4
            },]
        );
    }

    #[test]
    fn scales_css_spans_outward_for_high_dpi_windows() {
        let scaled = scale_spans(
            &[RegionSpan {
                left: 1,
                top: 2,
                right: 3,
                bottom: 4,
            }],
            1.5,
        );
        assert_eq!(
            scaled,
            vec![RegionSpan {
                left: 1,
                top: 3,
                right: 5,
                bottom: 6
            }]
        );
    }
}
