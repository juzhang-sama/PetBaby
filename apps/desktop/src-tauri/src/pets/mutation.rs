use std::collections::HashSet;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub type SharedPetMutationGate = Arc<PetMutationGate>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationKind {
    Switch,
    Creation,
    Delete,
    ProfileEdit,
}

#[derive(Clone, PartialEq, Eq)]
struct MutationOwner {
    request_id: String,
    kind: MutationKind,
    pet_id: String,
    started_at: Duration,
    scoped: bool,
    token: u64,
    pins: usize,
}

#[derive(Default)]
struct MutationState {
    owner: Option<MutationOwner>,
    profile_owners: Vec<MutationOwner>,
    next_token: u64,
    retired_cross_window_request_ids: HashSet<String>,
}

pub struct PetMutationGate {
    state: Mutex<MutationState>,
    changed: Condvar,
    timeout: Duration,
    clock: Arc<dyn Fn() -> Duration + Send + Sync>,
}

pub struct PetMutationLease<'a> {
    gate: &'a PetMutationGate,
    request_id: String,
    token: u64,
}

pub struct PetMutationOwnerPin<'a> {
    gate: &'a PetMutationGate,
    token: u64,
}

impl PetMutationOwnerPin<'_> {
    pub fn token(&self) -> u64 {
        self.token
    }
}

impl PetMutationGate {
    pub fn new(timeout: Duration) -> Self {
        let epoch = Instant::now();
        Self::new_with_clock(timeout, move || epoch.elapsed())
    }

    fn new_with_clock(
        timeout: Duration,
        clock: impl Fn() -> Duration + Send + Sync + 'static,
    ) -> Self {
        Self {
            state: Mutex::new(MutationState::default()),
            changed: Condvar::new(),
            timeout,
            clock: Arc::new(clock),
        }
    }

    pub fn begin(&self, request_id: &str, kind: MutationKind, pet_id: &str) -> Result<(), String> {
        self.acquire(request_id, kind, pet_id).map(|_| ())
    }

    pub fn scoped(
        &self,
        request_id: &str,
        kind: MutationKind,
        pet_id: &str,
    ) -> Result<PetMutationLease<'_>, String> {
        if request_id.is_empty() || pet_id.is_empty() {
            return Err("pet mutation request and target must not be empty".into());
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| "pet mutation gate lock poisoned")?;
        let token = loop {
            let now = (self.clock)();
            Self::retire_stale_cross_window_owners(&mut state, now, self.timeout);
            match Self::conflicting_owner(&state, kind, pet_id)
                .map(|owner| (owner.scoped, owner.pins, owner.started_at))
            {
                None => {
                    state.next_token = state.next_token.wrapping_add(1).max(1);
                    let token = state.next_token;
                    let owner = MutationOwner {
                        request_id: request_id.into(),
                        kind,
                        pet_id: pet_id.into(),
                        started_at: now,
                        scoped: true,
                        token,
                        pins: 0,
                    };
                    if kind == MutationKind::ProfileEdit {
                        state.profile_owners.push(owner);
                    } else {
                        state.owner = Some(owner);
                    }
                    break token;
                }
                Some((true, _, _)) => {
                    state = self
                        .changed
                        .wait(state)
                        .map_err(|_| "pet mutation gate lock poisoned")?;
                }
                Some((_, pins, _)) if pins > 0 => {
                    state = self
                        .changed
                        .wait(state)
                        .map_err(|_| "pet mutation gate lock poisoned")?;
                }
                Some((_, _, started_at)) => {
                    let age = now.saturating_sub(started_at);
                    let remaining = self.timeout.saturating_sub(age);
                    let (next, _) = self
                        .changed
                        .wait_timeout(state, remaining)
                        .map_err(|_| "pet mutation gate lock poisoned")?;
                    state = next;
                }
            }
        };
        drop(state);
        Ok(PetMutationLease {
            gate: self,
            request_id: request_id.into(),
            token,
        })
    }

    pub fn assert_owner(
        &self,
        request_id: &str,
        kind: MutationKind,
        pet_id: &str,
    ) -> Result<PetMutationOwnerPin<'_>, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "pet mutation gate lock poisoned")?;
        let MutationState {
            owner,
            profile_owners,
            ..
        } = &mut *state;
        let owner = owner
            .iter_mut()
            .chain(profile_owners.iter_mut())
            .find(|owner| {
                !owner.scoped
                    && owner.request_id == request_id
                    && owner.kind == kind
                    && owner.pet_id == pet_id
            })
            .ok_or_else(|| "pet mutation request does not own the expected gate".to_string())?;
        owner.pins = owner
            .pins
            .checked_add(1)
            .ok_or_else(|| "pet mutation owner pin overflow".to_string())?;
        Ok(PetMutationOwnerPin {
            gate: self,
            token: owner.token,
        })
    }

    pub fn finish(&self, request_id: &str) -> Result<Option<u64>, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "pet mutation gate lock poisoned")?;
        if state
            .owner
            .as_ref()
            .is_some_and(|owner| owner.request_id == request_id)
        {
            if state
                .owner
                .as_ref()
                .is_some_and(|owner| owner.scoped || owner.pins > 0)
            {
                return Err("pet mutation request does not own the gate".into());
            }
            let owner = state.owner.take().expect("matching owner must exist");
            let token = owner.token;
            state
                .retired_cross_window_request_ids
                .insert(owner.request_id);
            self.changed.notify_all();
            Ok(Some(token))
        } else if let Some(index) = state
            .profile_owners
            .iter()
            .position(|owner| owner.request_id == request_id)
        {
            if state.profile_owners[index].scoped || state.profile_owners[index].pins > 0 {
                return Err("pet mutation request does not own the gate".into());
            }
            let owner = state.profile_owners.remove(index);
            let token = owner.token;
            state
                .retired_cross_window_request_ids
                .insert(owner.request_id);
            self.changed.notify_all();
            Ok(Some(token))
        } else if state.owner.is_some() || !state.profile_owners.is_empty() {
            Err("pet mutation request does not own the gate".into())
        } else {
            Ok(None)
        }
    }

    fn acquire(&self, request_id: &str, kind: MutationKind, pet_id: &str) -> Result<u64, String> {
        if request_id.is_empty() || pet_id.is_empty() {
            return Err("pet mutation request and target must not be empty".into());
        }
        let now = (self.clock)();
        let mut state = self
            .state
            .lock()
            .map_err(|_| "pet mutation gate lock poisoned")?;
        Self::retire_stale_cross_window_owners(&mut state, now, self.timeout);
        if let Some(owner) = state
            .owner
            .iter()
            .chain(state.profile_owners.iter())
            .find(|owner| owner.request_id == request_id)
        {
            if owner.request_id == request_id
                && owner.kind == kind
                && owner.pet_id == pet_id
                && !owner.scoped
            {
                return Ok(owner.token);
            }
            return Err("已有宠物变更正在进行".into());
        }
        if Self::conflicting_owner(&state, kind, pet_id).is_some() {
            return Err("已有宠物变更正在进行".into());
        }
        if state.retired_cross_window_request_ids.contains(request_id) {
            return Err("pet mutation request id has already been retired".into());
        }
        state.next_token = state.next_token.wrapping_add(1).max(1);
        let token = state.next_token;
        let owner = MutationOwner {
            request_id: request_id.into(),
            kind,
            pet_id: pet_id.into(),
            started_at: now,
            scoped: false,
            token,
            pins: 0,
        };
        if kind == MutationKind::ProfileEdit {
            state.profile_owners.push(owner);
        } else {
            state.owner = Some(owner);
        }
        Ok(token)
    }

    fn conflicting_owner<'a>(
        state: &'a MutationState,
        kind: MutationKind,
        pet_id: &str,
    ) -> Option<&'a MutationOwner> {
        state
            .owner
            .iter()
            .chain(state.profile_owners.iter())
            .find(|owner| {
                (owner.kind != MutationKind::ProfileEdit && kind != MutationKind::ProfileEdit)
                    || owner.pet_id == pet_id
            })
    }

    fn retire_stale_cross_window_owners(
        state: &mut MutationState,
        now: Duration,
        timeout: Duration,
    ) {
        if state.owner.as_ref().is_some_and(|owner| {
            !owner.scoped && owner.pins == 0 && now.saturating_sub(owner.started_at) >= timeout
        }) {
            let owner = state.owner.take().expect("stale owner must exist");
            state
                .retired_cross_window_request_ids
                .insert(owner.request_id);
        }
        let mut retained = Vec::with_capacity(state.profile_owners.len());
        for owner in state.profile_owners.drain(..) {
            if !owner.scoped && owner.pins == 0 && now.saturating_sub(owner.started_at) >= timeout {
                state
                    .retired_cross_window_request_ids
                    .insert(owner.request_id);
            } else {
                retained.push(owner);
            }
        }
        state.profile_owners = retained;
    }

    fn finish_scoped(&self, request_id: &str, token: u64) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.owner.as_ref().is_some_and(|owner| {
            owner.scoped && owner.request_id == request_id && owner.token == token
        }) {
            state.owner = None;
            self.changed.notify_all();
            return;
        }
        if let Some(index) = state.profile_owners.iter().position(|owner| {
            owner.scoped && owner.request_id == request_id && owner.token == token
        }) {
            state.profile_owners.remove(index);
            self.changed.notify_all();
        }
    }

    fn unpin(&self, token: u64) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let MutationState {
            owner,
            profile_owners,
            ..
        } = &mut *state;
        if let Some(owner) = owner
            .iter_mut()
            .chain(profile_owners.iter_mut())
            .find(|owner| owner.token == token && !owner.scoped && owner.pins > 0)
        {
            owner.pins -= 1;
            self.changed.notify_all();
        }
    }
}

impl Drop for PetMutationLease<'_> {
    fn drop(&mut self) {
        self.gate.finish_scoped(&self.request_id, self.token);
    }
}

impl Drop for PetMutationOwnerPin<'_> {
    fn drop(&mut self) {
        self.gate.unpin(self.token);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    fn gate_with_clock(timeout: Duration) -> (PetMutationGate, Arc<AtomicU64>) {
        let now = Arc::new(AtomicU64::new(0));
        let clock = now.clone();
        let gate = PetMutationGate::new_with_clock(timeout, move || {
            Duration::from_secs(clock.load(Ordering::SeqCst))
        });
        (gate, now)
    }

    fn assert_current_owner(gate: &PetMutationGate, request_id: &str, pet_id: &str) {
        let state = gate.state.lock().unwrap();
        let owner = state.owner.as_ref().expect("expected mutation owner");
        assert_eq!(owner.request_id, request_id);
        assert_eq!(owner.pet_id, pet_id);
    }

    fn assert_scoped_waits_for(first_kind: MutationKind, waiting_kind: MutationKind) {
        let gate = Arc::new(PetMutationGate::new(Duration::from_secs(1)));
        let first = gate.scoped("first", first_kind, "pet-a").unwrap();
        let waiting = gate.clone();
        let (sent, received) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let _waiting = waiting.scoped("waiting", waiting_kind, "pet-a").unwrap();
            sent.send(()).unwrap();
        });

        assert!(received.recv_timeout(Duration::from_millis(20)).is_err());
        drop(first);
        assert_eq!(received.recv_timeout(Duration::from_secs(1)), Ok(()));
        thread.join().unwrap();
    }

    fn assert_scoped_does_not_wait_for_different_pet(
        first_kind: MutationKind,
        concurrent_kind: MutationKind,
    ) {
        let gate = Arc::new(PetMutationGate::new(Duration::from_secs(1)));
        let _first = gate.scoped("first", first_kind, "pet-a").unwrap();
        let concurrent = gate.clone();
        let (sent, received) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let _concurrent = concurrent
                .scoped("concurrent", concurrent_kind, "pet-b")
                .unwrap();
            sent.send(()).unwrap();
        });

        assert_eq!(received.recv_timeout(Duration::from_secs(1)), Ok(()));
        thread.join().unwrap();
    }

    #[test]
    fn profile_edit_conflicts_with_delete_for_same_pet() {
        assert_scoped_waits_for(MutationKind::Delete, MutationKind::ProfileEdit);
        assert_scoped_waits_for(MutationKind::ProfileEdit, MutationKind::Delete);
    }

    #[test]
    fn profile_edit_conflicts_with_switch_and_creation_for_same_pet_in_both_directions() {
        for other in [MutationKind::Switch, MutationKind::Creation] {
            assert_scoped_waits_for(other, MutationKind::ProfileEdit);
            assert_scoped_waits_for(MutationKind::ProfileEdit, other);
        }
    }

    #[test]
    fn profile_edits_for_the_same_pet_are_serialized() {
        assert_scoped_waits_for(MutationKind::ProfileEdit, MutationKind::ProfileEdit);
    }

    #[test]
    fn profile_edit_and_other_mutations_for_different_pets_do_not_interlock() {
        for other in [
            MutationKind::Switch,
            MutationKind::Delete,
            MutationKind::Creation,
        ] {
            assert_scoped_does_not_wait_for_different_pet(other, MutationKind::ProfileEdit);
            assert_scoped_does_not_wait_for_different_pet(MutationKind::ProfileEdit, other);
        }
        assert_scoped_does_not_wait_for_different_pet(
            MutationKind::ProfileEdit,
            MutationKind::ProfileEdit,
        );
    }

    #[test]
    fn profile_edit_lease_releases_when_the_operation_returns_an_error() {
        let gate = PetMutationGate::new(Duration::from_secs(1));
        let operation = || -> Result<(), String> {
            let _lease = gate.scoped("edit-failing", MutationKind::ProfileEdit, "pet-a")?;
            Err("injected repository failure".into())
        };

        assert_eq!(operation(), Err("injected repository failure".into()));
        drop(
            gate.scoped("delete-after-error", MutationKind::Delete, "pet-a")
                .unwrap(),
        );
    }

    #[test]
    fn one_request_owns_the_pet_mutation_gate_until_finish() {
        let gate = PetMutationGate::new(Duration::from_secs(60));
        gate.begin("req-a", MutationKind::Switch, "pet-a").unwrap();
        assert!(gate.begin("req-b", MutationKind::Delete, "pet-b").is_err());
        assert_current_owner(&gate, "req-a", "pet-a");
        gate.finish("req-a").unwrap();
        assert!(gate.begin("req-b", MutationKind::Delete, "pet-b").is_ok());
    }

    #[test]
    fn same_request_can_reenter_but_cannot_change_kind_or_target() {
        let gate = PetMutationGate::new(Duration::from_secs(60));
        gate.begin("req-a", MutationKind::Creation, "pet-a")
            .unwrap();
        gate.begin("req-a", MutationKind::Creation, "pet-a")
            .unwrap();
        assert!(gate
            .begin("req-a", MutationKind::Creation, "pet-b")
            .is_err());
        assert!(gate.begin("req-a", MutationKind::Delete, "pet-a").is_err());
    }

    #[test]
    fn stale_cross_window_owner_can_be_recovered() {
        let (gate, now) = gate_with_clock(Duration::from_secs(60));
        gate.begin("req-old", MutationKind::Switch, "pet-a")
            .unwrap();
        now.store(61, Ordering::SeqCst);

        gate.begin("req-new", MutationKind::Delete, "pet-b")
            .unwrap();
        assert_current_owner(&gate, "req-new", "pet-b");
    }

    #[test]
    fn cross_window_owner_is_recoverable_at_the_exact_timeout_boundary() {
        let (gate, now) = gate_with_clock(Duration::from_secs(60));
        gate.begin("req-old", MutationKind::Switch, "pet-a")
            .unwrap();

        now.store(60, Ordering::SeqCst);

        gate.begin("req-new", MutationKind::Delete, "pet-b")
            .unwrap();
        assert_current_owner(&gate, "req-new", "pet-b");
    }

    #[test]
    fn active_scoped_lease_cannot_be_recovered_by_ttl() {
        let (gate, now) = gate_with_clock(Duration::from_secs(60));
        let _lease = gate
            .scoped("job-a", MutationKind::Creation, "pet-a")
            .unwrap();
        now.store(600, Ordering::SeqCst);

        assert!(gate
            .begin("req-new", MutationKind::Switch, "pet-b")
            .is_err());
        assert_current_owner(&gate, "job-a", "pet-a");
    }

    #[test]
    fn scoped_operations_wait_and_resume_in_order() {
        let gate = Arc::new(PetMutationGate::new(Duration::from_secs(60)));
        let first = gate
            .scoped("job-a", MutationKind::Creation, "pet-a")
            .unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let waiting_gate = gate.clone();
        let waiter = std::thread::spawn(move || {
            let lease = waiting_gate
                .scoped("delete-a", MutationKind::Delete, "pet-a")
                .unwrap();
            tx.send(()).unwrap();
            drop(lease);
        });

        assert!(rx.recv_timeout(Duration::from_millis(20)).is_err());
        drop(first);
        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        waiter.join().unwrap();
    }

    #[test]
    fn stale_owner_finish_cannot_release_the_new_owner() {
        let (gate, now) = gate_with_clock(Duration::from_secs(60));
        gate.begin("req-old", MutationKind::Switch, "pet-a")
            .unwrap();
        now.store(61, Ordering::SeqCst);
        gate.begin("req-new", MutationKind::Delete, "pet-b")
            .unwrap();

        assert!(gate.finish("req-old").is_err());
        assert_current_owner(&gate, "req-new", "pet-b");
    }

    #[test]
    fn old_scoped_lease_drop_cannot_release_a_later_owner() {
        let gate = PetMutationGate::new(Duration::from_secs(60));
        let old_lease = gate
            .scoped("job-old", MutationKind::Creation, "pet-a")
            .unwrap();
        let old_token = old_lease.token;
        assert!(gate.finish("job-old").is_err());
        gate.finish_scoped("job-old", old_token);
        gate.begin("req-new", MutationKind::Switch, "pet-b")
            .unwrap();

        drop(old_lease);

        assert_current_owner(&gate, "req-new", "pet-b");
    }

    #[test]
    fn scoped_lease_releases_on_error_and_panic_without_releasing_a_later_owner() {
        let gate = PetMutationGate::new(Duration::from_secs(60));
        let operation = || -> Result<(), String> {
            let _lease = gate.scoped("job-error", MutationKind::Creation, "pet-a")?;
            Err("injected failure".into())
        };
        assert!(operation().is_err());
        gate.begin("req-after-error", MutationKind::Switch, "pet-b")
            .unwrap();
        gate.finish("req-after-error").unwrap();

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _lease = gate
                .scoped("job-panic", MutationKind::Delete, "pet-a")
                .unwrap();
            panic!("injected panic");
        }));
        assert!(panic.is_err());
        gate.begin("req-after-panic", MutationKind::Switch, "pet-c")
            .unwrap();
    }

    #[test]
    fn owner_pin_blocks_finish_and_ttl_recovery_until_drop() {
        let (gate, now) = gate_with_clock(Duration::from_secs(60));
        gate.begin("switch-a", MutationKind::Switch, "pet-a")
            .unwrap();
        let pin = gate
            .assert_owner("switch-a", MutationKind::Switch, "pet-a")
            .unwrap();
        now.store(600, Ordering::SeqCst);

        assert!(gate.finish("switch-a").is_err());
        assert!(gate
            .begin("delete-a", MutationKind::Delete, "pet-a")
            .is_err());

        drop(pin);
        gate.finish("switch-a").unwrap();
        gate.begin("delete-a", MutationKind::Delete, "pet-a")
            .unwrap();
    }

    #[test]
    fn owner_pin_rejects_kind_target_and_scoped_mismatches() {
        let gate = PetMutationGate::new(Duration::from_secs(60));
        gate.begin("switch-a", MutationKind::Switch, "pet-a")
            .unwrap();
        assert!(gate
            .assert_owner("switch-a", MutationKind::Creation, "pet-a")
            .is_err());
        assert!(gate
            .assert_owner("switch-a", MutationKind::Switch, "pet-b")
            .is_err());
        gate.finish("switch-a").unwrap();

        let _lease = gate
            .scoped("local-a", MutationKind::Switch, "pet-a")
            .unwrap();
        assert!(gate
            .assert_owner("local-a", MutationKind::Switch, "pet-a")
            .is_err());
    }

    #[test]
    fn finished_cross_window_request_id_is_retired_for_the_process_lifetime() {
        let gate = PetMutationGate::new(Duration::from_secs(60));
        gate.begin("cross-once", MutationKind::Switch, "pet-a")
            .unwrap();
        gate.finish("cross-once").unwrap();

        assert!(gate
            .begin("cross-once", MutationKind::Switch, "pet-a")
            .is_err());
        assert!(gate
            .begin("cross-new", MutationKind::Switch, "pet-a")
            .is_ok());
    }

    #[test]
    fn ttl_recovered_request_id_is_retired_but_a_new_id_can_start() {
        let (gate, now) = gate_with_clock(Duration::from_secs(60));
        gate.begin("cross-stale", MutationKind::Switch, "pet-a")
            .unwrap();
        now.store(60, Ordering::SeqCst);

        assert!(gate
            .begin("cross-stale", MutationKind::Switch, "pet-a")
            .is_err());
        assert!(gate
            .begin("cross-after-stale", MutationKind::Delete, "pet-b")
            .is_ok());
    }

    #[test]
    fn scoped_request_ids_can_be_reused_even_after_a_cross_window_retirement() {
        let gate = PetMutationGate::new(Duration::from_secs(60));
        gate.begin("shared-local-id", MutationKind::Switch, "pet-a")
            .unwrap();
        gate.finish("shared-local-id").unwrap();

        drop(
            gate.scoped("shared-local-id", MutationKind::Creation, "pet-a")
                .unwrap(),
        );
        drop(
            gate.scoped("shared-local-id", MutationKind::Creation, "pet-a")
                .unwrap(),
        );
    }

    #[test]
    fn delayed_retired_finish_cannot_release_a_new_request_owner() {
        let gate = PetMutationGate::new(Duration::from_secs(60));
        gate.begin("cross-old", MutationKind::Switch, "pet-a")
            .unwrap();
        gate.finish("cross-old").unwrap();
        gate.begin("cross-new", MutationKind::Delete, "pet-b")
            .unwrap();

        assert!(gate.finish("cross-old").is_err());
        assert_current_owner(&gate, "cross-new", "pet-b");
    }

    #[test]
    fn owner_pin_drops_on_panic_without_finishing_the_owner() {
        let gate = PetMutationGate::new(Duration::from_secs(60));
        gate.begin("switch-a", MutationKind::Switch, "pet-a")
            .unwrap();

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _pin = gate
                .assert_owner("switch-a", MutationKind::Switch, "pet-a")
                .unwrap();
            panic!("injected pin panic");
        }));

        assert!(panic.is_err());
        gate.finish("switch-a").unwrap();
    }

    #[test]
    fn old_owner_pin_drop_cannot_modify_a_later_owner() {
        let gate = PetMutationGate::new(Duration::from_secs(60));
        gate.begin("switch-old", MutationKind::Switch, "pet-a")
            .unwrap();
        let old_pin = gate
            .assert_owner("switch-old", MutationKind::Switch, "pet-a")
            .unwrap();
        gate.state.lock().unwrap().owner = None;
        gate.begin("switch-new", MutationKind::Switch, "pet-b")
            .unwrap();

        drop(old_pin);

        let _new_pin = gate
            .assert_owner("switch-new", MutationKind::Switch, "pet-b")
            .unwrap();
    }
}
