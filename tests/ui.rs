#![allow(dead_code)] // The executable is imported to exercise its private UI logic.

#[path = "../src/main.rs"]
mod app;

use osfr_launcher::auth::AuthError;
use std::{cell::Cell, path::PathBuf, rc::Rc};

const SERVER_IDENTITY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

#[test]
fn join_flow_saves_profile_lists_it_and_copies_the_public_key() {
    let public_key = "ab".repeat(32);
    let mut generated_for = None;
    let mut saved_profile = None;
    let joined = app::create_join_profile_with(
        "127.0.0.1:20042",
        "127.0.0.1:20043",
        SERVER_IDENTITY,
        "acct_test",
        "join-test",
        |id| {
            generated_for = Some(id.to_owned());
            Ok(public_key.clone())
        },
        |profile| {
            saved_profile = Some(profile.clone());
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(generated_for.as_deref(), Some("join-test"));
    assert_eq!(saved_profile.as_ref(), Some(&joined.profile));
    assert_eq!(
        joined.profile.server_identity,
        app::parse_server_identity(SERVER_IDENTITY).unwrap()
    );
    assert_eq!(
        app::short_fingerprint(&joined.profile.server_identity),
        "0011-2233-4455-6677"
    );

    let mut servers = Vec::new();
    let mut profiles = Vec::new();
    let selected = app::register_joined_profile(&joined, &mut servers, &mut profiles);
    assert_eq!(servers[selected].profile_id, "join-test");
    assert_eq!(
        profiles,
        vec![("join-test".into(), joined.profile.display_name.clone())]
    );
    assert_eq!(
        app::profile_label(&servers[selected].profile_id, &profiles),
        joined.profile.display_name
    );

    let mut copied = None;
    app::copy_public_key(&joined.public_key, |text| copied = Some(text));
    assert_eq!(copied.as_deref(), Some(public_key.as_str()));
}

#[test]
fn malformed_pin_does_not_generate_or_save_a_key() {
    let mut key_called = false;
    let mut save_called = false;
    let result = app::create_join_profile_with(
        "127.0.0.1:20042",
        "127.0.0.1:20043",
        "not-a-pin",
        "acct_test",
        "join-test",
        |_| {
            key_called = true;
            Ok("ab".repeat(32))
        },
        |_| {
            save_called = true;
            Ok(())
        },
    );

    assert!(result.is_err());
    assert!(!key_called);
    assert!(!save_called);
}

#[test]
fn settings_render_fingerprint_copy_button_and_host_instruction() {
    let context = eframe::egui::Context::default();
    let creation = eframe::CreationContext::_new_kittest(context.clone());
    let mut launcher = app::Launcher::new(
        &creation,
        Some(PathBuf::from("preview.png")),
        true,
        Rc::new(Cell::new(false)),
        false,
        Some(1.0),
    )
    .unwrap();
    launcher.load_join_preview();
    let output = context.run(
        eframe::egui::RawInput {
            screen_rect: Some(eframe::egui::Rect::from_min_size(
                eframe::egui::Pos2::ZERO,
                eframe::egui::Vec2::new(900.0, 700.0),
            )),
            focused: true,
            ..Default::default()
        },
        |context| {
            eframe::egui::CentralPanel::default().show(context, |ui| {
                launcher.join_realm_ui(ui, true);
            });
        },
    );

    let visible = format!("{:?}", output.shapes);
    assert!(visible.contains("Short fingerprint to compare: 0011-2233-4455-6677"));
    assert!(
        visible.contains("Send this public key to your host so they can add it to your account.")
    );
    let actual: Vec<_> = launcher
        .focus_targets
        .iter()
        .map(|(target, _)| *target)
        .collect();
    assert_eq!(
        actual,
        [
            app::FocusTarget::JoinAddress,
            app::FocusTarget::JoinBroker,
            app::FocusTarget::JoinIdentity,
            app::FocusTarget::JoinHandle,
            app::FocusTarget::CreateKey,
            app::FocusTarget::CopyPublicKey,
        ]
    );
}

#[test]
fn new_join_controls_and_conditional_cancel_are_in_focus_order() {
    let ready = app::focus_order(true, 1, false, true);
    for control in [
        app::FocusTarget::JoinAddress,
        app::FocusTarget::JoinBroker,
        app::FocusTarget::JoinIdentity,
        app::FocusTarget::JoinHandle,
        app::FocusTarget::CreateKey,
        app::FocusTarget::CopyPublicKey,
    ] {
        assert!(ready.contains(&control), "missing {control:?}");
    }

    let mut canary = ready.clone();
    canary.retain(|target| *target != app::FocusTarget::CreateKey);
    assert!(!app::focus_sequence_matches(&canary, true, 1, false, true));

    let idle = app::focus_order(true, 1, false, false);
    let patching = app::focus_order(true, 1, true, false);
    assert!(!idle.contains(&app::FocusTarget::CancelPatch));
    assert!(patching.contains(&app::FocusTarget::CancelPatch));
}

#[test]
fn key_generation_errors_keep_auth_error_messages() {
    assert_eq!(
        app::auth_error_message(AuthError::RandomUnavailable),
        "The operating system could not generate a secure key."
    );
}

#[test]
fn broker_defaults_to_the_realm_host_and_refuses_public_hosts() {
    assert_eq!(app::default_broker("10.0.0.5:20042"), "10.0.0.5:20043");
    assert_eq!(app::default_broker("[fd12::7]:20042"), "[fd12::7]:20043");
    assert_eq!(app::default_broker("not an address"), "127.0.0.1:20043");
    for (broker, ok) in [("100.64.0.9:20043", true), ("8.8.8.8:20043", false)] {
        let result = app::create_join_profile_with(
            "100.64.0.9:20042",
            broker,
            SERVER_IDENTITY,
            "acct_test",
            "join-test",
            |_| Ok("ab".repeat(32)),
            |_| Ok(()),
        );
        assert_eq!(result.is_ok(), ok, "{broker}");
        if let Ok(joined) = result {
            assert_eq!(joined.profile.broker_endpoint, broker);
        }
    }
}
