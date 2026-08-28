use dogmos_core::{
	metadata::{
		GameplayHandle, GasFireRole, GasId, GasMetadata, GasRequirement, NativeReactionKind,
		ReactionExecution, ReactionId, ReactionMetadata, TurfHandle,
	},
	world::{
		Command, CommandResult, DogmosWorld, LifecycleAction, LifecycleMutation,
		MixtureStateMutation, ReactionContinuationToken, ReactionProgress, StageResult,
		TurfLifecycleMutation, WorldError, WorldEvent, WorldStage,
	},
	MixtureHandle, MAX_GAS_SLOTS,
};

fn gas(id: u16, key: &str) -> GasMetadata {
	GasMetadata {
		id: GasId(id),
		key: key.into(),
		name: key.into(),
		flags: 0,
		specific_heat: 20.0,
		fusion_power: 0.0,
		moles_visible: None,
		enthalpy: 0.0,
		fire_radiation_released: 0.0,
		fire_role: GasFireRole::None,
		fire_products: None,
	}
}

#[test]
fn direct_reaction_preserves_arbitrary_holder_across_dm_continuation_without_a_turf() {
	let mixture = MixtureHandle {
		slot: 0,
		generation: 1,
	};
	let holder = GameplayHandle {
		slot: 41,
		generation: 9,
	};
	let mut world = DogmosWorld::new_with_event_capacity(1024 * 1024, 8);
	world
		.install_gases(vec![
			gas(0, "o2"),
			gas(1, "hydrogen"),
			gas(2, "water_vapor"),
		])
		.unwrap();
	world
		.install_reactions(vec![
			ReactionMetadata {
				id: ReactionId(0),
				key: "dm_first".into(),
				priority: 2.0,
				minimum_temperature: None,
				maximum_temperature: None,
				minimum_energy: None,
				minimum_fire_reagents: None,
				gas_requirements: Box::new([]),
				execution: ReactionExecution::Dm,
			},
			ReactionMetadata {
				id: ReactionId(1),
				key: "h2fire".into(),
				priority: 1.0,
				minimum_temperature: None,
				maximum_temperature: None,
				minimum_energy: None,
				minimum_fire_reagents: None,
				gas_requirements: vec![
					GasRequirement {
						gas: GasId(0),
						minimum_moles: 0.01,
					},
					GasRequirement {
						gas: GasId(1),
						minimum_moles: 0.01,
					},
				]
				.into_boxed_slice(),
				execution: ReactionExecution::Native(NativeReactionKind::Hydrogen),
			},
		])
		.unwrap();
	world
		.apply_lifecycle(&[LifecycleMutation {
			action: LifecycleAction::Register,
			handle: mixture,
		}])
		.unwrap();
	let mut gases = [0.0; MAX_GAS_SLOTS];
	gases[0] = 100.0;
	gases[1] = 10.0;
	world
		.apply_mixture_state(&[MixtureStateMutation {
			handle: mixture,
			expected_revision: 0,
			temperature: 500.0,
			volume: 2500.0,
			gases,
		}])
		.unwrap();

	assert_eq!(
		world.react_mixture_with_event_limit(mixture, holder, Some(0.0), 8),
		Ok(ReactionProgress {
			flags: 0,
			work_items: 1,
			pending: true,
		})
	);
	let mut events = Vec::new();
	assert_eq!(world.drain_events_into(8, &mut events), 1);
	let token = match events.as_slice() {
		[WorldEvent::RunDmReaction {
			turf: None,
			mixture: event_mixture,
			target,
			reaction: ReactionId(0),
			continuation,
		}] if *event_mixture == mixture && *target == holder => *continuation,
		actual => panic!("unexpected direct reaction event: {actual:?}"),
	};

	assert_eq!(
		world.resume_reaction_with_result_and_event_limit(token, 1, 8),
		Ok(ReactionProgress {
			flags: 5,
			work_items: 1,
			pending: false,
		})
	);
	assert_eq!(world.drain_events_into(8, &mut events), 2);
	assert!(matches!(
		events.as_slice(),
		[WorldEvent::ReactionFinished {
			mixture: event_mixture,
			target: event_target,
			reaction: ReactionId(1),
			kind: NativeReactionKind::Hydrogen,
			..
		}, WorldEvent::ReactionProfiled {
			mixture: profiled_mixture,
			target: profiled_target,
			reaction: ReactionId(1),
			cost_ms,
		}] if *event_mixture == mixture
			&& *event_target == holder
			&& *profiled_mixture == mixture
			&& *profiled_target == holder
			&& *cost_ms >= 0.0
	));
}

#[test]
fn dm_reaction_continuation_resumes_priority_order_and_rejects_duplicate_resume() {
	let mixture = MixtureHandle {
		slot: 0,
		generation: 1,
	};
	let turf = TurfHandle {
		slot: 0,
		generation: 1,
	};
	let mut world = DogmosWorld::new_with_event_capacity(1024 * 1024, 8);
	world
		.install_gases(vec![
			gas(0, "o2"),
			gas(1, "hydrogen"),
			gas(2, "water_vapor"),
		])
		.unwrap();
	world
		.install_reactions(vec![
			ReactionMetadata {
				id: ReactionId(0),
				key: "dm_first".into(),
				priority: 2.0,
				minimum_temperature: None,
				maximum_temperature: None,
				minimum_energy: None,
				minimum_fire_reagents: None,
				gas_requirements: Box::new([]),
				execution: ReactionExecution::Dm,
			},
			ReactionMetadata {
				id: ReactionId(1),
				key: "h2fire".into(),
				priority: 1.0,
				minimum_temperature: None,
				maximum_temperature: None,
				minimum_energy: None,
				minimum_fire_reagents: None,
				gas_requirements: vec![
					GasRequirement {
						gas: GasId(0),
						minimum_moles: 0.01,
					},
					GasRequirement {
						gas: GasId(1),
						minimum_moles: 0.01,
					},
				]
				.into_boxed_slice(),
				execution: ReactionExecution::Native(NativeReactionKind::Hydrogen),
			},
		])
		.unwrap();
	world
		.apply_lifecycle(&[LifecycleMutation {
			action: LifecycleAction::Register,
			handle: mixture,
		}])
		.unwrap();
	let mut gases = [0.0; MAX_GAS_SLOTS];
	gases[0] = 100.0;
	gases[1] = 10.0;
	world
		.apply_mixture_state(&[MixtureStateMutation {
			handle: mixture,
			expected_revision: 0,
			temperature: 500.0,
			volume: 2500.0,
			gases,
		}])
		.unwrap();
	world
		.apply_turf_lifecycle(&[TurfLifecycleMutation::Register {
			handle: turf,
			mixture: Some(mixture),
		}])
		.unwrap();

	assert_eq!(
		world
			.process_stage_cancellable(WorldStage::React, 0.5, || false)
			.unwrap(),
		StageResult { work_items: 1 }
	);
	let before_resume = world.snapshot(mixture).unwrap();
	let mut events = Vec::new();
	assert_eq!(world.drain_events_into(8, &mut events), 1);
	let token = match events.as_slice() {
		[WorldEvent::RunDmReaction {
			turf: Some(event_turf),
			mixture: event_mixture,
			target,
			reaction: ReactionId(0),
			continuation,
		}] if *event_turf == turf
			&& *target == GameplayHandle::from(turf)
			&& *event_mixture == mixture =>
		{
			*continuation
		}
		actual => panic!("unexpected continuation event: {actual:?}"),
	};
	assert_ne!(token, ReactionContinuationToken::default());
	assert_eq!(world.pending_reaction_continuations(), 1);
	assert_eq!(
		world.resume_reaction_with_event_limit(token, 0),
		Err(WorldError::EventCapacityExceeded {
			requested: 1,
			capacity: 0,
		})
	);
	assert_eq!(world.pending_reaction_continuations(), 1);
	assert_eq!(world.resume_reaction_with_event_limit(token, 8), Ok(1));
	assert_eq!(world.pending_reaction_continuations(), 0);
	let after_resume = world.snapshot(mixture).unwrap();
	assert!(after_resume.gases[1] < before_resume.gases[1]);
	assert_eq!(world.drain_events_into(8, &mut events), 1);
	assert!(matches!(
		events.as_slice(),
		[WorldEvent::ReactionFinished {
			reaction: ReactionId(1),
			kind: NativeReactionKind::Hydrogen,
			..
		}]
	));
	assert_eq!(
		world.apply_command(Command::ResumeReaction {
			continuation: token,
		}),
		Err(WorldError::UnknownReactionContinuation(token))
	);

	world
		.process_stage_cancellable(WorldStage::React, 0.5, || false)
		.unwrap();
	world.drain_events_into(8, &mut events);
	let cancelled = match events.as_slice() {
		[WorldEvent::RunDmReaction { continuation, .. }] => *continuation,
		actual => panic!("unexpected cancellation continuation event: {actual:?}"),
	};
	assert_eq!(world.cancel_reaction(cancelled), Ok(()));
	assert_eq!(world.pending_reaction_continuations(), 0);
	assert_eq!(
		world.cancel_reaction(cancelled),
		Err(WorldError::UnknownReactionContinuation(cancelled))
	);
}

#[test]
fn turf_lifecycle_invalidates_reaction_continuations_without_aba_reuse() {
	let mixture = MixtureHandle {
		slot: 0,
		generation: 1,
	};
	let original_turf = TurfHandle {
		slot: 0,
		generation: 1,
	};
	let replacement_turf = TurfHandle {
		slot: 0,
		generation: 2,
	};
	let mut world = DogmosWorld::new_with_event_capacity(1024 * 1024, 1);
	world.install_gases(vec![gas(0, "o2")]).unwrap();
	world
		.install_reactions(vec![ReactionMetadata {
			id: ReactionId(0),
			key: "dm_only".into(),
			priority: 1.0,
			minimum_temperature: None,
			maximum_temperature: None,
			minimum_energy: None,
			minimum_fire_reagents: None,
			gas_requirements: Box::new([]),
			execution: ReactionExecution::Dm,
		}])
		.unwrap();
	world
		.apply_lifecycle(&[LifecycleMutation {
			action: LifecycleAction::Register,
			handle: mixture,
		}])
		.unwrap();
	world
		.apply_turf_lifecycle(&[TurfLifecycleMutation::Register {
			handle: original_turf,
			mixture: Some(mixture),
		}])
		.unwrap();

	world
		.process_stage_cancellable(WorldStage::React, 0.5, || false)
		.unwrap();
	let mut events = Vec::new();
	world.drain_events_into(1, &mut events);
	let original_token = match events.as_slice() {
		[WorldEvent::RunDmReaction { continuation, .. }] => *continuation,
		actual => panic!("unexpected continuation event: {actual:?}"),
	};
	world
		.apply_turf_lifecycle(&[TurfLifecycleMutation::Unregister {
			handle: original_turf,
		}])
		.unwrap();
	assert_eq!(
		world.apply_command(Command::ResumeReaction {
			continuation: original_token,
		}),
		Err(WorldError::UnknownReactionContinuation(original_token))
	);

	world
		.apply_turf_lifecycle(&[TurfLifecycleMutation::Register {
			handle: replacement_turf,
			mixture: Some(mixture),
		}])
		.unwrap();
	world
		.process_stage_cancellable(WorldStage::React, 0.5, || false)
		.unwrap();
	world.drain_events_into(1, &mut events);
	let replacement_token = match events.as_slice() {
		[WorldEvent::RunDmReaction { continuation, .. }] => *continuation,
		actual => panic!("unexpected replacement continuation event: {actual:?}"),
	};
	assert_eq!(replacement_token.slot, original_token.slot);
	assert!(replacement_token.generation > original_token.generation);
	assert_eq!(
		world.apply_command(Command::ResumeReaction {
			continuation: original_token,
		}),
		Err(WorldError::StaleReactionContinuation {
			requested: original_token,
			current: replacement_token.generation,
		})
	);
	assert_eq!(
		world.apply_command(Command::ResumeReaction {
			continuation: replacement_token,
		}),
		Ok(CommandResult::Applied { updated: 0 })
	);
}

#[test]
fn plasma_kernel_mutates_service_state_and_emits_typed_finish_event() {
	let mixture = MixtureHandle {
		slot: 0,
		generation: 1,
	};
	let turf = TurfHandle {
		slot: 0,
		generation: 1,
	};
	let mut world = DogmosWorld::new_with_event_capacity(1024 * 1024, 8);
	world
		.install_gases(vec![
			gas(0, "o2"),
			gas(1, "plasma"),
			gas(2, "co2"),
			gas(3, "tritium"),
			gas(4, "water_vapor"),
		])
		.unwrap();
	world
		.install_reactions(vec![ReactionMetadata {
			id: ReactionId(0),
			key: "plasmafire".into(),
			priority: 1.0,
			minimum_temperature: Some(373.15),
			maximum_temperature: None,
			minimum_energy: None,
			minimum_fire_reagents: None,
			gas_requirements: vec![
				GasRequirement {
					gas: GasId(0),
					minimum_moles: 0.01,
				},
				GasRequirement {
					gas: GasId(1),
					minimum_moles: 0.01,
				},
			]
			.into_boxed_slice(),
			execution: ReactionExecution::Native(NativeReactionKind::Plasma),
		}])
		.unwrap();
	world
		.apply_lifecycle(&[LifecycleMutation {
			action: LifecycleAction::Register,
			handle: mixture,
		}])
		.unwrap();
	let mut gases = [0.0; MAX_GAS_SLOTS];
	gases[0] = 100.0;
	gases[1] = 9.0;
	world
		.apply_mixture_state(&[MixtureStateMutation {
			handle: mixture,
			expected_revision: 0,
			temperature: 1000.0,
			volume: 2500.0,
			gases,
		}])
		.unwrap();
	world
		.apply_turf_lifecycle(&[TurfLifecycleMutation::Register {
			handle: turf,
			mixture: Some(mixture),
		}])
		.unwrap();

	assert_eq!(
		world
			.process_stage_cancellable(WorldStage::React, 0.5, || false)
			.unwrap(),
		StageResult { work_items: 1 }
	);
	let after = world.snapshot(mixture).unwrap();
	assert!(after.gases[0] < 100.0);
	assert!(after.gases[1] < 9.0);
	assert!(after.gases[2] > 0.0 || after.gases[3] > 0.0);
	assert!(after.gases[4] > 0.0 || after.gases[3] > 0.0);
	assert!(after.temperature > 1000.0);

	let mut events = Vec::new();
	assert_eq!(world.drain_events_into(8, &mut events), 1);
	assert!(matches!(
		events.as_slice(),
		[WorldEvent::ReactionFinished {
			mixture: event_mixture,
			target,
			reaction: ReactionId(0),
			kind: NativeReactionKind::Plasma,
			..
		}] if *target == GameplayHandle::from(turf) && *event_mixture == mixture
	));
}
