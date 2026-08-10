use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub type SharedPetMutationGate = Arc<PetMutationGate>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationKind {
    Switch,
    Creation,
    Delete,
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
    next_token: u64,
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
            if state.owner.as_ref().is_some_and(|owner| {
                !owner.scoped
                    && owner.pins == 0
                    && now.saturating_sub(owner.started_at) > self.timeout
            }) {
                state.owner = None;
            }
            match state.owner.as_ref() {
                None => {
                    state.next_token = state.next_token.wrapping_add(1).max(1);
                    let token = state.next_token;
                    state.owner = Some(MutationOwner {
                        request_id: request_id.into(),
                        kind,
                        pet_id: pet_id.into(),
                        started_at: now,
                        scoped: true,
                        token,
                        pins: 0,
                    });
                    break token;
                }
                Some(owner) if owner.scoped => {
                    state = self
                        .changed
                        .wait(state)
                        .map_err(|_| "pet mutation gate lock poisoned")?;
                }
                Some(owner) if owner.pins > 0 => {
                    state = self
                        .changed
                        .wait(state)
                        .map_err(|_| "pet mutation gate lock poisoned")?;
                }
                Some(owner) => {
                    let age = now.saturating_sub(owner.started_at);
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
        let owner = state
            .owner
            .as_mut()
            .filter(|owner| {
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

    pub fn finish(&self, request_id: &str) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "pet mutation gate lock poisoned")?;
        match state.owner.as_ref() {
            None => Ok(()),
            Some(owner) if owner.request_id == request_id && !owner.scoped && owner.pins == 0 => {
                state.owner = None;
                self.changed.notify_all();
                Ok(())
            }
            Some(_) => Err("pet mutation request does not own the gate".into()),
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
        if state.owner.as_ref().is_some_and(|owner| {
            !owner.scoped && owner.pins == 0 && now.saturating_sub(owner.started_at) > self.timeout
        }) {
            state.owner = None;
        }
        if let Some(owner) = state.owner.as_ref() {
            if owner.request_id == request_id
                && owner.kind == kind
                && owner.pet_id == pet_id
                && !owner.scoped
            {
                return Ok(owner.token);
            }
            return Err("已有宠物变更正在进行".into());
        }
        state.next_token = state.next_token.wrapping_add(1).max(1);
        let token = state.next_token;
        state.owner = Some(MutationOwner {
            request_id: request_id.into(),
            kind,
            pet_id: pet_id.into(),
            started_at: now,
            scoped: false,
            token,
            pins: 0,
        });
        Ok(token)
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
        }
    }

    fn unpin(&self, token: u64) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if let Some(owner) = state
            .owner
            .as_mut()
            .filter(|owner| owner.token == token && !owner.scoped && owner.pins > 0)
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
