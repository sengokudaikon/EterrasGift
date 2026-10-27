use super::*;
use eg_core::game_log::LoggedCharacter;
use eg_trainer::backend::{BackendSnapshot, HOOK_NOT_AVAILABLE, HookRun, ProfileLock};
use eg_trainer::features::{DESCRIPTORS, FeatureKey, HookId};
use eg_trainer::profiles::{ProfileBook, ProfileKey, ProfileSaver};

fn hook(id: &str) -> HookId {
    HookId::parse(id).unwrap()
}

#[test]
fn rift_choice_changes_wanted_and_value_in_one_command_including_restoration() {
    run(async {
        let shared = Shared::new();
        playing(&shared, "Oren");
        shared.hooks_supported.store(true, Ordering::SeqCst);
        let (options, _) = saving(book());
        let (handle, worker) = start(&shared, options);
        connected(&handle).await;
        let id = hook("mysterious-rift");
        for choice in [1.0, 2.0, 1.0, 0.0] {
            command(&handle, Command::SetHookValues { id, values: vec![choice] }).await.unwrap();
            assert_eq!(handle.snapshot().hooks[&id], if choice == 0.0 { HookRun::Off } else { HookRun::On });
        }
        assert_eq!(
            requested(&shared),
            [(id, true, vec![1.0]), (id, true, vec![2.0]), (id, true, vec![1.0]), (id, false, vec![0.0])]
        );
        command(&handle, Command::SetHookValues { id, values: vec![3.0] }).await.unwrap_err();
        assert_eq!(shared.count("hook"), 4);
        shutdown(&handle, worker).await;
    });
}
fn key(name: &str) -> ProfileKey {
    ProfileKey {
        name: name.into(),
        class: "Primalist".into(),
        cycle: "Legacy".into(),
    }
}
fn playing(shared: &Shared, name: &str) {
    shared.log.lock().unwrap().character = Some(LoggedCharacter {
        name: name.into(),
        level: Some(12),
        online: false,
        cycle: "Legacy".into(),
        class: "Primalist".into(),
    });
}
/// Mira wants God mode, Damage dealt (at 3) and Zoom; Oren, played before her, wants nothing.
fn book() -> ProfileBook {
    ProfileBook::from_settings(Some(&serde_json::json!({
        "profiles": [
            { "name": "Oren", "class": "Primalist", "cycle": "Legacy", "lastPlayed": "2026-10-17T09:00:00Z", "wanted": [] },
            {
                "name": "Mira", "class": "Primalist", "cycle": "Legacy", "lastPlayed": "2026-10-18T09:00:00Z",
                "wanted": ["god-mode", "damage-dealt", "zoom"], "values": { "damage-dealt": [3.0] }
            }
        ]
    })))
}
/// Mira's wanted hooks in the order the pages list them, which is the order they are applied in.
fn mira_order() -> Vec<HookId> {
    DESCRIPTORS
        .iter()
        .filter_map(|descriptor| match descriptor.key {
            FeatureKey::Hook(id) if ["god-mode", "damage-dealt", "zoom"].contains(&id.as_str()) => Some(id),
            _ => None,
        })
        .collect()
}
/// Options with these profiles, whose saves are kept, the latest last.
fn saving(book: ProfileBook) -> (BackendOptions, Arc<Mutex<Vec<ProfileBook>>>) {
    let saves = Arc::new(Mutex::new(vec![]));
    let kept = saves.clone();
    let options = BackendOptions {
        profiles: book,
        save_profiles: Some(ProfileSaver::new(move |book| kept.lock().unwrap().push(book.clone()))),
        ..options()
    };
    (options, saves)
}
async fn wait_until(handle: &BackendHandle, done: impl Fn(&BackendSnapshot) -> bool) {
    let mut receiver = handle.subscribe();
    while !done(&receiver.borrow_and_update()) {
        receiver.changed().await.unwrap();
    }
}
fn requested(shared: &Shared) -> Vec<(HookId, bool, Vec<f64>)> {
    shared
        .hook_requests
        .lock()
        .unwrap()
        .iter()
        .map(|request| (request.id, request.on, request.values.clone()))
        .collect()
}

#[test]
fn a_character_without_a_profile_has_hooks_for_this_connection_until_they_are_kept() {
    run(async {
        let shared = Shared::new();
        playing(&shared, "Ysolde");
        shared.hooks_supported.store(true, Ordering::SeqCst);
        let (options, saves) = saving(book());
        let (handle, worker) = start(&shared, options);
        assert_eq!(handle.snapshot().editing, Some(key("Mira")), "the pages open on the last played");
        connected(&handle).await;
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.profile_lock, ProfileLock::Live(key("Ysolde")));
        assert_eq!(snapshot.editing, Some(key("Ysolde")));
        assert_eq!(snapshot.profiles.profiles().len(), 2, "playing makes no profile");
        assert_eq!(snapshot.shown_hooks(), Some(snapshot.profiles.template()));
        assert!(!snapshot.hooks.is_empty() && snapshot.hooks.values().all(|run| *run == HookRun::Off));
        assert_eq!(shared.count("hook"), 0);

        let zoom = hook("zoom");
        command(&handle, Command::SetHookWanted { id: zoom, on: true }).await.unwrap();
        command(&handle, Command::SetHookValues { id: zoom, values: vec![-20.] }).await.unwrap();
        assert_eq!(requested(&shared).last(), Some(&(zoom, true, vec![-20.])));
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.hooks[&zoom], HookRun::On);
        assert!(snapshot.shown_hooks().unwrap().is_wanted(zoom));
        assert!(saves.lock().unwrap().is_empty(), "this connection's hooks are not saved");

        command(&handle, Command::CreateProfile).await.unwrap();
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.unsaved, None);
        let names: Vec<_> = snapshot.profiles.profiles().iter().map(|profile| profile.key.name.as_str()).collect();
        assert_eq!(names, ["Ysolde", "Mira", "Oren"]);
        let ysolde = snapshot.profiles.get(&key("Ysolde")).unwrap();
        assert_eq!(
            (ysolde.last_played, ysolde.hooks.is_wanted(zoom), ysolde.hooks.values(zoom)),
            (Some(now()), true, &[-20.][..])
        );
        assert_eq!(saves.lock().unwrap().last(), Some(&*snapshot.profiles));
        assert_eq!(command(&handle, Command::CreateProfile).await.unwrap_err().kind, ErrorKind::InvalidEdit);
        assert_eq!(shared.count("hook"), 2, "keeping them changes nothing in the game");
        command(&handle, Command::SetHookWanted { id: zoom, on: false }).await.unwrap();
        assert!(
            !saves.lock().unwrap().last().unwrap().get(&key("Ysolde")).unwrap().hooks.is_wanted(zoom),
            "and changes are saved from now on"
        );
        shutdown(&handle, worker).await;
    });
}

#[test]
fn deleting_the_profile_in_play_leaves_its_hooks_to_this_connection_and_they_end_with_it() {
    run(async {
        let shared = Shared::new();
        playing(&shared, "Mira");
        shared.hooks_supported.store(true, Ordering::SeqCst);
        let (options, saves) = saving(book());
        let (handle, worker) = start(&shared, options);
        connected(&handle).await;
        let applied = shared.count("hook");
        assert_eq!(
            command(&handle, Command::DeleteProfile(key("Oren"))).await.unwrap_err().kind,
            ErrorKind::NotReady,
            "only the one in play"
        );
        assert_eq!(command(&handle, Command::CreateProfile).await.unwrap_err().kind, ErrorKind::InvalidEdit, "Mira has one");
        command(&handle, Command::DeleteProfile(key("Mira"))).await.unwrap();
        let snapshot = handle.snapshot();
        assert!(snapshot.profiles.get(&key("Mira")).is_none());
        assert!(saves.lock().unwrap().last().unwrap().get(&key("Mira")).is_none());
        assert_eq!((&snapshot.profile_lock, &snapshot.editing), (&ProfileLock::Live(key("Mira")), &Some(key("Mira"))));
        let shown = snapshot.shown_hooks().unwrap();
        assert!(mira_order().iter().all(|id| shown.is_wanted(*id) && snapshot.hooks[id] == HookRun::On));
        assert_eq!(shared.count("hook"), applied, "nothing changes in the game");

        command(&handle, Command::Disconnect).await.unwrap();
        let snapshot = handle.snapshot();
        assert_eq!((snapshot.unsaved, snapshot.editing), (None, Some(key("Oren"))));
        shutdown(&handle, worker).await;
    });
}

#[test]
fn wanted_hooks_apply_in_turn_and_one_the_game_refuses_does_not_stop_the_rest() {
    run(async {
        let shared = Shared::new();
        playing(&shared, "Mira");
        shared.hooks_supported.store(true, Ordering::SeqCst);
        let order = mira_order();
        shared.hook_results.lock().unwrap().extend([
            Ok(WriteOutcome::Confirmed(true)),
            Ok(WriteOutcome::refused("The game has nothing to hook here.")),
            Ok(WriteOutcome::NotKept(false)),
        ]);
        let (options, _) = saving(book());
        let (handle, worker) = start(&shared, options);
        connected(&handle).await;
        let requests = requested(&shared);
        assert_eq!(requests.iter().map(|request| request.0).collect::<Vec<_>>(), order);
        assert!(requests.iter().all(|request| request.1));
        let damage = requests.iter().find(|request| request.0 == hook("damage-dealt")).unwrap();
        assert_eq!(damage.2, [3.0], "the profile's values go with the hook");
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.hooks[&order[0]], HookRun::On);
        assert_eq!(
            snapshot.hooks[&order[1]],
            HookRun::Failed {
                reason: "The game has nothing to hook here.".into(),
                on: false
            }
        );
        assert!(matches!(&snapshot.hooks[&order[2]], HookRun::Failed { on: false, .. }), "its report says off");
        assert_eq!((snapshot.phase, snapshot.error), (Phase::Ready, None));
        shutdown(&handle, worker).await;
    });
}

#[test]
fn an_uncertain_hook_stops_the_rest_and_releases_the_game_until_retry() {
    run(async {
        let shared = Shared::new();
        playing(&shared, "Mira");
        shared.hooks_supported.store(true, Ordering::SeqCst);
        let order = mira_order();
        shared
            .hook_results
            .lock()
            .unwrap()
            .extend([Ok(WriteOutcome::Confirmed(true)), Ok(WriteOutcome::uncertain("no answer after sending"))]);
        let (handle, worker) = start(&shared, saving(book()).0);
        command(&handle, Command::Enable).await.unwrap();
        let error = command(&handle, Command::Connect).await.unwrap_err();
        assert_eq!(error.kind, ErrorKind::WriteUncertain);
        let title = FeatureKey::Hook(order[1]).descriptor().title;
        assert_eq!(error.message, format!("{title}: no answer after sending"));
        assert_eq!(shared.count("hook"), 2, "nothing after the uncertain one");
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.phase, Phase::Blocked);
        assert!(snapshot.hooks.is_empty(), "the helper left with its hooks");
        assert_eq!(snapshot.profile_lock, ProfileLock::Locked);
        assert_eq!(shared.count("disconnect"), 1);

        command(&handle, Command::Connect).await.unwrap();
        assert_eq!(shared.count("hook"), 2, "never again without Retry");
        command(&handle, Command::Retry).await.unwrap();
        assert_eq!(shared.count("hook"), 5);
        assert!(order.iter().all(|id| handle.snapshot().hooks[id] == HookRun::On));
        shutdown(&handle, worker).await;
    });
}

#[test]
fn a_hook_this_version_cannot_apply_says_so_without_reaching_the_game() {
    run(async {
        let shared = Shared::new();
        playing(&shared, "Mira");
        let (handle, worker) = start(&shared, saving(book()).0);
        connected(&handle).await;
        let snapshot = handle.snapshot();
        for id in mira_order() {
            assert_eq!(
                snapshot.hooks[&id],
                HookRun::Failed {
                    reason: HOOK_NOT_AVAILABLE.into(),
                    on: false
                }
            );
        }
        assert_eq!(snapshot.hooks[&hook("one-hit-kill")], HookRun::Off);
        assert_eq!(shared.count("hook"), 0);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn profiles_change_while_no_game_runs_and_lock_while_one_does_without_opening_it() {
    run(async {
        let shared = Shared::new();
        shared.targets.lock().unwrap().clear();
        let (options, saves) = saving(book());
        let (handle, worker) = start(&shared, options);
        command(&handle, Command::Enable).await.unwrap();
        command(&handle, Command::StartMonitoring).await.unwrap();
        wait_until(&handle, |snapshot| snapshot.profile_lock == ProfileLock::Editable).await;
        command(
            &handle,
            Command::SetHookWanted {
                id: hook("one-hit-kill"),
                on: true,
            },
        )
        .await
        .unwrap();
        let saved = saves.lock().unwrap().last().cloned().unwrap();
        assert!(saved.get(&key("Mira")).unwrap().hooks.is_wanted(hook("one-hit-kill")));

        // Automatic connection is off: the game is only looked for.
        shared.targets.lock().unwrap().push(target());
        wait_until(&handle, |snapshot| snapshot.profile_lock == ProfileLock::Locked).await;
        assert_eq!(handle.snapshot().detected, Some(target()));
        for refused in [
            Command::SetHookWanted { id: hook("zoom"), on: false },
            Command::SetHookValues {
                id: hook("zoom"),
                values: vec![-20.],
            },
            Command::SelectProfile(key("Oren")),
            Command::DeleteProfile(key("Oren")),
        ] {
            assert_eq!(command(&handle, refused).await.unwrap_err().kind, ErrorKind::NotReady);
        }
        assert_eq!((shared.count("gate"), shared.count("connect")), (0, 0), "looking opens nothing");
        assert_eq!(handle.snapshot().profiles.profiles().len(), 2);

        shared.targets.lock().unwrap().clear();
        wait_until(&handle, |snapshot| snapshot.profile_lock == ProfileLock::Editable).await;
        shutdown(&handle, worker).await;
    });
}

#[test]
fn a_live_change_is_saved_and_applied_and_turning_a_hook_off_takes_it_back() {
    run(async {
        let shared = Shared::new();
        playing(&shared, "Oren");
        shared.hooks_supported.store(true, Ordering::SeqCst);
        let (options, saves) = saving(book());
        let (handle, worker) = start(&shared, options);
        connected(&handle).await;
        let zoom = hook("zoom");
        command(&handle, Command::SetHookValues { id: zoom, values: vec![-20.] }).await.unwrap();
        assert_eq!(shared.count("hook"), 0, "a hook that is off takes its values when it is turned on");
        command(&handle, Command::SetHookWanted { id: zoom, on: true }).await.unwrap();
        command(&handle, Command::SetHookValues { id: zoom, values: vec![-30.] }).await.unwrap();
        assert_eq!(
            command(&handle, Command::SetHookValues { id: zoom, values: vec![-30., 1.] }).await.unwrap_err().kind,
            ErrorKind::InvalidEdit
        );
        assert_eq!(handle.snapshot().hooks[&zoom], HookRun::On);
        command(&handle, Command::SetHookWanted { id: zoom, on: false }).await.unwrap();
        assert_eq!(requested(&shared), [(zoom, true, vec![-20.]), (zoom, true, vec![-30.]), (zoom, false, vec![-30.])]);
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.hooks[&zoom], HookRun::Off);
        let saved = saves.lock().unwrap().last().cloned().unwrap();
        let oren = saved.get(&key("Oren")).unwrap();
        assert_eq!((oren.hooks.is_wanted(zoom), oren.hooks.values(zoom)), (false, &[-30.][..]));
        assert_eq!(
            command(&handle, Command::SelectProfile(key("Mira"))).await.unwrap_err().kind,
            ErrorKind::NotReady,
            "the character in play is the one shown"
        );
        shutdown(&handle, worker).await;
    });
}

#[test]
fn a_chosen_profile_is_the_one_changed_and_deleting_it_shows_the_last_played() {
    run(async {
        let shared = Shared::new();
        let (options, saves) = saving(book());
        let (handle, worker) = start(&shared, options);
        command(&handle, Command::SelectProfile(key("Oren"))).await.unwrap();
        command(&handle, Command::SetHookWanted { id: hook("god-mode"), on: true }).await.unwrap();
        let snapshot = handle.snapshot();
        assert!(snapshot.profiles.get(&key("Oren")).unwrap().hooks.is_wanted(hook("god-mode")));
        assert_eq!(snapshot.profiles.get(&key("Mira")).unwrap().hooks.wanted().count(), 3);
        assert_eq!(command(&handle, Command::SelectProfile(key("Ysolde"))).await.unwrap_err().kind, ErrorKind::InvalidEdit);
        command(&handle, Command::DeleteProfile(key("Oren"))).await.unwrap();
        assert_eq!(handle.snapshot().editing, Some(key("Mira")));
        assert!(saves.lock().unwrap().last().unwrap().get(&key("Oren")).is_none());
        command(&handle, Command::DeleteProfile(key("Mira"))).await.unwrap();
        assert_eq!(handle.snapshot().editing, None);
        assert_eq!(
            command(&handle, Command::SetHookWanted { id: hook("god-mode"), on: true }).await.unwrap_err().kind,
            ErrorKind::NotReady,
            "no profile to change"
        );
        shutdown(&handle, worker).await;
    });
}

#[test]
fn hooks_refused_while_the_character_loads_wait_and_go_on_once_it_is_in_play() {
    run(async {
        let shared = Shared::new();
        playing(&shared, "Mira");
        shared.hooks_supported.store(true, Ordering::SeqCst);
        let order = mira_order();
        shared
            .hook_results
            .lock()
            .unwrap()
            .push_back(Err(BackendError::waiting(ErrorKind::CharacterLoading, "the character is loading")));
        let (handle, worker) = start(&shared, saving(book()).0);
        command(&handle, Command::Enable).await.unwrap();
        assert_eq!(command(&handle, Command::Connect).await.unwrap_err().kind, ErrorKind::CharacterLoading);
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.phase, Phase::WaitingForCharacter);
        assert_eq!(snapshot.profile_lock, ProfileLock::Locked);
        assert!(order.iter().all(|id| snapshot.hooks[id] == HookRun::Queued));

        command(&handle, Command::StartMonitoring).await.unwrap();
        wait_until(&handle, |snapshot| order.iter().all(|id| snapshot.hooks[id] == HookRun::On)).await;
        assert_eq!(requested(&shared).iter().filter(|request| request.0 == order[0]).count(), 2);
        assert_eq!(handle.snapshot().profile_lock, ProfileLock::Live(key("Mira")));
        shutdown(&handle, worker).await;
    });
}

#[test]
fn independently_observed_suspension_requeues_wanted_hooks_on_the_monitor() {
    run(async {
        let shared = Shared::new();
        playing(&shared, "Mira");
        shared.hooks_supported.store(true, Ordering::SeqCst);
        let (handle, worker) = start(&shared, saving(book()).0);
        connected(&handle).await;
        let before = requested(&shared);
        assert_eq!(before.len(), mira_order().len());
        shared.hooks_suspended.store(true, Ordering::SeqCst);
        command(&handle, Command::StartMonitoring).await.unwrap();
        wait_until(&handle, |snapshot| {
            requested(&shared).len() == 2 * before.len() && mira_order().iter().all(|id| snapshot.hooks[id] == HookRun::On)
        })
        .await;
        assert_eq!(&requested(&shared)[before.len()..], before);
        shutdown(&handle, worker).await;
    });
}
