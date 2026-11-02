use super::*;

#[test]
fn installs_alpine_packages_and_enables_openrc_service_in_order() {
    let mut calls = Vec::new();
    let mut phases = Vec::new();
    install_apk_with(
        &mut |program, args| {
            calls.push((
                program.to_owned(),
                args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            ));
            Ok(())
        },
        &mut |phase| phases.push(phase),
    )
    .unwrap();
    assert_eq!(
        calls,
        vec![
            (
                "apk".into(),
                vec![
                    "add",
                    "--no-cache",
                    "docker",
                    "docker-openrc",
                    "docker-cli-buildx",
                    "docker-cli-compose"
                ]
                .into_iter()
                .map(String::from)
                .collect()
            ),
            (
                "rc-update".into(),
                vec!["add", "docker", "default"]
                    .into_iter()
                    .map(String::from)
                    .collect()
            ),
            (
                "rc-service".into(),
                vec!["docker", "start"]
                    .into_iter()
                    .map(String::from)
                    .collect()
            ),
            ("docker".into(), vec!["info".into()]),
        ]
    );
    assert_eq!(
        phases,
        vec![
            InstallPhase::Preparing,
            InstallPhase::InstallingPackages,
            InstallPhase::StartingService
        ]
    );
}

#[test]
fn openrc_docker_readiness_retries_without_real_processes_or_sleep() {
    let mut calls = 0;
    let mut waits = 0;
    wait_for_docker(
        &mut || {
            calls += 1;
            if calls < 3 {
                Err(SoftwareError::Message("not ready".into()))
            } else {
                Ok(())
            }
        },
        &mut || waits += 1,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!((calls, waits), (3, 2));
}

#[test]
fn openrc_docker_readiness_is_bounded_and_cancellable() {
    let mut calls = 0;
    let mut waits = 0;
    assert!(
        wait_for_docker(
            &mut || {
                calls += 1;
                Err(SoftwareError::Message("not ready".into()))
            },
            &mut || waits += 1,
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert_eq!((calls, waits), (30, 29));
    let cancel = AtomicBool::new(true);
    assert!(
        wait_for_docker(
            &mut || panic!("cancelled probe must not run"),
            &mut || panic!("must not wait"),
            &cancel
        )
        .is_err()
    );
    cancel.store(false, Ordering::Relaxed);
    calls = 0;
    assert!(
        wait_for_docker(
            &mut || {
                calls += 1;
                Err(SoftwareError::Message("not ready".into()))
            },
            &mut || cancel.store(true, Ordering::Relaxed),
            &cancel
        )
        .is_err()
    );
    assert_eq!(calls, 1);
}

#[test]
fn stops_after_each_failed_install_or_service_command() {
    for fail_at in 0..4 {
        let mut calls = 0;
        let result = install_apk_with(
            &mut |_, _| {
                let current = calls;
                calls += 1;
                if current == fail_at {
                    Err(SoftwareError::Message("fixture failure".into()))
                } else {
                    Ok(())
                }
            },
            &mut |_| {},
        );
        assert!(result.is_err());
        assert_eq!(calls, fail_at + 1);
    }
}

#[test]
fn alpine_only_offers_system_repositories_and_rejects_ce_mirrors_before_install() {
    assert_eq!(
        DockerSource::available(Family::Alpine),
        vec![DockerSource::Official]
    );
    assert!(DockerSource::Official.validate(Family::Alpine).is_ok());
    let host = Host {
        family: Family::Alpine,
        codename: None,
    };
    for source in DockerSource::all()
        .into_iter()
        .filter(|s| *s != DockerSource::Official)
    {
        let mut phases = Vec::new();
        assert!(
            install(
                &host,
                source,
                false,
                &AtomicBool::new(false),
                &mut |phase| phases.push(phase)
            )
            .is_err()
        );
        assert!(phases.is_empty());
    }
}
