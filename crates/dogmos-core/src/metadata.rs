use crate::MAX_GAS_SLOTS;
use std::{collections::BTreeMap, error::Error, fmt};

const MINIMUM_FIRE_MOLES: f32 = 0.0001;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct GasId(pub u16);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct ReactionId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(C)]
pub struct TurfHandle {
	pub slot: u32,
	pub generation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GasFireRole {
	Oxidizer {
		minimum_temperature: f32,
		power: f32,
	},
	Fuel {
		minimum_temperature: f32,
		burn_rate: f32,
	},
	None,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GasProduct {
	pub gas: GasId,
	pub ratio: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GasRequirement {
	pub gas: GasId,
	pub minimum_moles: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeReactionKind {
	Plasma,
	Hydrogen,
	Tritium,
	Freon,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReactionExecution {
	Native(NativeReactionKind),
	Dm,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FireProductRule {
	Generic(Box<[GasProduct]>),
	Plasma,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GasMetadata {
	pub id: GasId,
	pub key: Box<str>,
	pub name: Box<str>,
	pub flags: u32,
	pub specific_heat: f32,
	pub fusion_power: f32,
	pub moles_visible: Option<f32>,
	pub enthalpy: f32,
	pub fire_radiation_released: f32,
	pub fire_role: GasFireRole,
	pub fire_products: Option<FireProductRule>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReactionMetadata {
	pub id: ReactionId,
	pub key: Box<str>,
	pub priority: f32,
	pub minimum_temperature: Option<f32>,
	pub maximum_temperature: Option<f32>,
	pub minimum_energy: Option<f32>,
	pub minimum_fire_reagents: Option<f32>,
	pub gas_requirements: Box<[GasRequirement]>,
	pub execution: ReactionExecution,
}

#[derive(Clone, Debug)]
pub struct GasMetadataRegistry {
	gases: Box<[GasMetadata]>,
	ids_by_key: BTreeMap<Box<str>, GasId>,
	specific_heats: Box<[f32]>,
}

#[derive(Clone, Debug)]
pub struct ReactionMetadataRegistry {
	reactions: Box<[ReactionMetadata]>,
	ids_by_key: BTreeMap<Box<str>, ReactionId>,
	priority_order: Box<[ReactionId]>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GasMetadataError {
	TooManyGases { count: u32, maximum: u32 },
	DuplicateGasId(GasId),
	NonDenseGasId { expected: GasId, actual: GasId },
	EmptyGasKey(GasId),
	DuplicateGasKey(Box<str>),
	InvalidSpecificHeat(GasId),
	InvalidFusionPower(GasId),
	InvalidMolesVisible(GasId),
	InvalidEnthalpy(GasId),
	InvalidFireRadiation(GasId),
	InvalidFireRole(GasId),
	UnknownFireProduct { gas: GasId, product: GasId },
	InvalidFireProductRatio { gas: GasId, product: GasId },
	DuplicateFireProduct { gas: GasId, product: GasId },
}

#[derive(Clone, Debug, PartialEq)]
pub enum ReactionMetadataError {
	DuplicateReactionId(ReactionId),
	NonDenseReactionId {
		expected: ReactionId,
		actual: ReactionId,
	},
	EmptyReactionKey(ReactionId),
	DuplicateReactionKey(Box<str>),
	InvalidPriority(ReactionId),
	DuplicateReactionPriority {
		first: ReactionId,
		second: ReactionId,
	},
	InvalidMinimumTemperature(ReactionId),
	InvalidMaximumTemperature(ReactionId),
	InvalidTemperatureRange(ReactionId),
	InvalidMinimumEnergy(ReactionId),
	InvalidMinimumFireReagents(ReactionId),
	UnknownRequiredGas {
		reaction: ReactionId,
		gas: GasId,
	},
	InvalidRequiredMoles {
		reaction: ReactionId,
		gas: GasId,
	},
	DuplicateRequiredGas {
		reaction: ReactionId,
		gas: GasId,
	},
}

impl fmt::Display for GasMetadataError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(formatter, "{self:?}")
	}
}

impl Error for GasMetadataError {}

impl fmt::Display for ReactionMetadataError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(formatter, "{self:?}")
	}
}

impl Error for ReactionMetadataError {}

impl GasMetadataRegistry {
	pub fn try_new(mut gases: Vec<GasMetadata>) -> Result<Self, GasMetadataError> {
		let count = u32::try_from(gases.len()).unwrap_or(u32::MAX);
		let maximum = MAX_GAS_SLOTS as u32;
		if count > maximum {
			return Err(GasMetadataError::TooManyGases { count, maximum });
		}

		gases.sort_unstable_by_key(|gas| gas.id);
		if let Some(duplicate) = gases
			.windows(2)
			.find_map(|pair| (pair[0].id == pair[1].id).then_some(pair[0].id))
		{
			return Err(GasMetadataError::DuplicateGasId(duplicate));
		}

		let mut ids_by_key = BTreeMap::new();
		for (expected, gas) in (0_u16..).zip(&gases) {
			let expected = GasId(expected);
			if gas.id != expected {
				return Err(GasMetadataError::NonDenseGasId {
					expected,
					actual: gas.id,
				});
			}
			if gas.key.is_empty() {
				return Err(GasMetadataError::EmptyGasKey(gas.id));
			}
			if ids_by_key.insert(gas.key.clone(), gas.id).is_some() {
				return Err(GasMetadataError::DuplicateGasKey(gas.key.clone()));
			}
			validate_gas(gas, count)?;
		}

		let specific_heats = gases
			.iter()
			.map(|gas| gas.specific_heat)
			.collect::<Vec<_>>()
			.into_boxed_slice();
		Ok(Self {
			gases: gases.into_boxed_slice(),
			ids_by_key,
			specific_heats,
		})
	}

	pub fn len(&self) -> u32 {
		self.gases.len() as u32
	}

	pub fn is_empty(&self) -> bool {
		self.gases.is_empty()
	}

	pub fn by_id(&self, id: GasId) -> Option<&GasMetadata> {
		self.gases.get(usize::from(id.0))
	}

	pub fn by_key(&self, key: &str) -> Option<&GasMetadata> {
		self.ids_by_key.get(key).and_then(|id| self.by_id(*id))
	}

	pub fn specific_heats(&self) -> &[f32] {
		&self.specific_heats
	}

	pub fn iter(&self) -> impl Iterator<Item = &GasMetadata> {
		self.gases.iter()
	}
}

impl ReactionMetadataRegistry {
	pub fn try_new(
		mut reactions: Vec<ReactionMetadata>,
		gases: &GasMetadataRegistry,
	) -> Result<Self, ReactionMetadataError> {
		reactions.sort_unstable_by_key(|reaction| reaction.id);
		if let Some(duplicate) = reactions
			.windows(2)
			.find_map(|pair| (pair[0].id == pair[1].id).then_some(pair[0].id))
		{
			return Err(ReactionMetadataError::DuplicateReactionId(duplicate));
		}

		let mut ids_by_key = BTreeMap::new();
		let mut ids_by_priority_bits = BTreeMap::new();
		for (expected, reaction) in (0_u32..).zip(&reactions) {
			let expected = ReactionId(expected);
			if reaction.id != expected {
				return Err(ReactionMetadataError::NonDenseReactionId {
					expected,
					actual: reaction.id,
				});
			}
			if reaction.key.is_empty() {
				return Err(ReactionMetadataError::EmptyReactionKey(reaction.id));
			}
			if ids_by_key
				.insert(reaction.key.clone(), reaction.id)
				.is_some()
			{
				return Err(ReactionMetadataError::DuplicateReactionKey(
					reaction.key.clone(),
				));
			}
			validate_reaction(reaction, gases)?;
			let priority_bits = if reaction.priority == 0.0 {
				0
			} else {
				reaction.priority.to_bits()
			};
			if let Some(first) = ids_by_priority_bits.insert(priority_bits, reaction.id) {
				return Err(ReactionMetadataError::DuplicateReactionPriority {
					first,
					second: reaction.id,
				});
			}
		}

		let mut priority_order = reactions
			.iter()
			.map(|reaction| reaction.id)
			.collect::<Vec<_>>();
		priority_order.sort_unstable_by(|left, right| {
			reactions[usize::try_from(right.0).expect("u32 fits usize")]
				.priority
				.total_cmp(&reactions[usize::try_from(left.0).expect("u32 fits usize")].priority)
		});

		Ok(Self {
			reactions: reactions.into_boxed_slice(),
			ids_by_key,
			priority_order: priority_order.into_boxed_slice(),
		})
	}

	pub fn len(&self) -> u32 {
		u32::try_from(self.reactions.len()).unwrap_or(u32::MAX)
	}

	pub fn is_empty(&self) -> bool {
		self.reactions.is_empty()
	}

	pub fn by_id(&self, id: ReactionId) -> Option<&ReactionMetadata> {
		self.reactions.get(usize::try_from(id.0).ok()?)
	}

	pub fn by_key(&self, key: &str) -> Option<&ReactionMetadata> {
		self.ids_by_key.get(key).and_then(|id| self.by_id(*id))
	}

	pub fn priority_order(&self) -> &[ReactionId] {
		&self.priority_order
	}

	pub(crate) fn reactable_ids_into(
		&self,
		temperature: f32,
		moles: &[f32; MAX_GAS_SLOTS],
		gases: &GasMetadataRegistry,
		output: &mut Vec<ReactionId>,
	) {
		let mut thermal_energy = None;
		let mut fire_reagents = None;
		for id in &self.priority_order {
			let reaction = self
				.by_id(*id)
				.expect("priority order contains only registered reaction ids");
			if !reaction_conditions_met(
				reaction,
				temperature,
				moles,
				gases,
				&mut thermal_energy,
				&mut fire_reagents,
			) {
				continue;
			}
			output.push(*id);
		}
	}

	pub(crate) fn is_reactable(
		&self,
		id: ReactionId,
		temperature: f32,
		moles: &[f32; MAX_GAS_SLOTS],
		gases: &GasMetadataRegistry,
	) -> bool {
		let Some(reaction) = self.by_id(id) else {
			return false;
		};
		reaction_conditions_met(reaction, temperature, moles, gases, &mut None, &mut None)
	}
}

fn validate_gas(gas: &GasMetadata, gas_count: u32) -> Result<(), GasMetadataError> {
	if !gas.specific_heat.is_finite() || gas.specific_heat <= 0.0 {
		return Err(GasMetadataError::InvalidSpecificHeat(gas.id));
	}
	if !gas.fusion_power.is_finite() || gas.fusion_power < 0.0 {
		return Err(GasMetadataError::InvalidFusionPower(gas.id));
	}
	if gas
		.moles_visible
		.is_some_and(|value| !value.is_finite() || value < 0.0)
	{
		return Err(GasMetadataError::InvalidMolesVisible(gas.id));
	}
	if !gas.enthalpy.is_finite() {
		return Err(GasMetadataError::InvalidEnthalpy(gas.id));
	}
	if !gas.fire_radiation_released.is_finite() || gas.fire_radiation_released < 0.0 {
		return Err(GasMetadataError::InvalidFireRadiation(gas.id));
	}
	if !valid_fire_role(gas.fire_role) {
		return Err(GasMetadataError::InvalidFireRole(gas.id));
	}
	if let Some(FireProductRule::Generic(products)) = &gas.fire_products {
		let mut seen = [false; MAX_GAS_SLOTS];
		for product in products {
			if u32::from(product.gas.0) >= gas_count {
				return Err(GasMetadataError::UnknownFireProduct {
					gas: gas.id,
					product: product.gas,
				});
			}
			if !product.ratio.is_finite() || product.ratio < 0.0 {
				return Err(GasMetadataError::InvalidFireProductRatio {
					gas: gas.id,
					product: product.gas,
				});
			}
			let product_index = usize::from(product.gas.0);
			if seen[product_index] {
				return Err(GasMetadataError::DuplicateFireProduct {
					gas: gas.id,
					product: product.gas,
				});
			}
			seen[product_index] = true;
		}
	}
	Ok(())
}

fn validate_reaction(
	reaction: &ReactionMetadata,
	gases: &GasMetadataRegistry,
) -> Result<(), ReactionMetadataError> {
	if !reaction.priority.is_finite() {
		return Err(ReactionMetadataError::InvalidPriority(reaction.id));
	}
	if reaction
		.minimum_temperature
		.is_some_and(|value| !value.is_finite() || value < 0.0)
	{
		return Err(ReactionMetadataError::InvalidMinimumTemperature(
			reaction.id,
		));
	}
	if reaction
		.maximum_temperature
		.is_some_and(|value| !value.is_finite() || value < 0.0)
	{
		return Err(ReactionMetadataError::InvalidMaximumTemperature(
			reaction.id,
		));
	}
	if reaction
		.minimum_temperature
		.zip(reaction.maximum_temperature)
		.is_some_and(|(minimum, maximum)| minimum > maximum)
	{
		return Err(ReactionMetadataError::InvalidTemperatureRange(reaction.id));
	}
	if reaction
		.minimum_energy
		.is_some_and(|value| !value.is_finite() || value < 0.0)
	{
		return Err(ReactionMetadataError::InvalidMinimumEnergy(reaction.id));
	}
	if reaction
		.minimum_fire_reagents
		.is_some_and(|value| !value.is_finite() || value < 0.0)
	{
		return Err(ReactionMetadataError::InvalidMinimumFireReagents(
			reaction.id,
		));
	}

	let mut seen = [false; MAX_GAS_SLOTS];
	for requirement in &reaction.gas_requirements {
		if gases.by_id(requirement.gas).is_none() {
			return Err(ReactionMetadataError::UnknownRequiredGas {
				reaction: reaction.id,
				gas: requirement.gas,
			});
		}
		if !requirement.minimum_moles.is_finite() || requirement.minimum_moles < 0.0 {
			return Err(ReactionMetadataError::InvalidRequiredMoles {
				reaction: reaction.id,
				gas: requirement.gas,
			});
		}
		let gas_index = usize::from(requirement.gas.0);
		if seen[gas_index] {
			return Err(ReactionMetadataError::DuplicateRequiredGas {
				reaction: reaction.id,
				gas: requirement.gas,
			});
		}
		seen[gas_index] = true;
	}
	Ok(())
}

fn reaction_conditions_met(
	reaction: &ReactionMetadata,
	temperature: f32,
	moles: &[f32; MAX_GAS_SLOTS],
	gases: &GasMetadataRegistry,
	thermal_energy: &mut Option<f32>,
	fire_reagents: &mut Option<f32>,
) -> bool {
	reaction
		.minimum_temperature
		.is_none_or(|minimum| temperature >= minimum)
		&& reaction
			.maximum_temperature
			.is_none_or(|maximum| temperature <= maximum)
		&& reaction
			.gas_requirements
			.iter()
			.all(|requirement| moles[usize::from(requirement.gas.0)] >= requirement.minimum_moles)
		&& reaction.minimum_energy.is_none_or(|minimum| {
			*thermal_energy.get_or_insert_with(|| {
				moles
					.iter()
					.zip(gases.specific_heats())
					.fold(0.0, |capacity, (amount, specific_heat)| {
						specific_heat.mul_add(*amount, capacity)
					}) * temperature
			}) >= minimum
		}) && reaction.minimum_fire_reagents.is_none_or(|minimum| {
		*fire_reagents.get_or_insert_with(|| {
			let (oxidation_power, fuel_amount) =
				moles
					.iter()
					.zip(&gases.gases)
					.fold((0.0, 0.0), |mut totals, (amount, gas)| {
						if *amount <= MINIMUM_FIRE_MOLES {
							return totals;
						}
						match gas.fire_role {
							GasFireRole::Oxidizer {
								minimum_temperature,
								power,
							} if temperature > minimum_temperature => {
								let available =
									amount * (1.0 - minimum_temperature / temperature).max(0.0);
								totals.0 += available * power;
							}
							GasFireRole::Fuel {
								minimum_temperature,
								burn_rate,
							} if temperature > minimum_temperature => {
								let available =
									amount * (1.0 - minimum_temperature / temperature).max(0.0);
								totals.1 += available / burn_rate;
							}
							_ => {}
						}
						totals
					});
			oxidation_power.min(fuel_amount)
		}) >= minimum
	})
}

fn valid_fire_role(role: GasFireRole) -> bool {
	match role {
		GasFireRole::Oxidizer {
			minimum_temperature,
			power,
		} => {
			minimum_temperature.is_finite()
				&& minimum_temperature >= 0.0
				&& power.is_finite()
				&& power >= 0.0
		}
		GasFireRole::Fuel {
			minimum_temperature,
			burn_rate,
		} => {
			minimum_temperature.is_finite()
				&& minimum_temperature >= 0.0
				&& burn_rate.is_finite()
				&& burn_rate > 0.0
		}
		GasFireRole::None => true,
	}
}
