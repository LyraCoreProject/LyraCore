use spacetimedb::table;

/// A Form composed from the Module's building blocks. See docs/schema.md "Forms".
#[table(accessor = game_form, public)]
#[derive(Clone)]
pub struct Form {
    #[primary_key]
    pub form_id: u32,
    pub name: String,
    /// The spell whose Aura the Module applies when it puts a unit in this Form itself.
    pub spell_id: u32,
    pub acts_as_shapeshifted: bool,
    /// `lyracore_shared::packing::power_type`, the pool the power rule applies to.
    pub power_type: u8,
    /// A `lyracore_shared::form::PowerOnEntry` code.
    pub power_on_entry: u8,
    /// The class a unit with no Form enters this Form at world entry; zero is none.
    pub default_for_class: u8,
    /// The 1.12 client's form byte, `UNIT_FIELD_BYTES_1` byte 2.
    pub client_form: u8,
    /// Zero is the weapon combat profile; else weapons are set aside for natural attacks that
    /// swing this often.
    #[default(0u32)]
    pub natural_attack_ms: u32,
    /// Agility adds to the melee attack power of a druid in the Form.
    #[default(false)]
    pub attack_power_from_agility: bool,
    /// Entering and leaving the Form ends the unit's roots and snares.
    #[default(false)]
    pub clears_snares: bool,
    /// The spell entering the Form casts at the unit's `A_FORM_ENTRY_CHANCE`; zero is none.
    #[default(0u32)]
    pub entry_chance_spell: u32,
}

/// A passive spell a unit holds while in a Form.
#[table(
    accessor = game_form_spell,
    public,
    index(accessor = by_form, btree(columns = [form_id])),
    index(accessor = by_spell, btree(columns = [spell_id]))
)]
pub struct FormSpell {
    /// `lyracore_package_delta::packed_form_spell_id(form_id, spell_id)`.
    #[primary_key]
    pub id: u64,
    pub form_id: u32,
    pub spell_id: u32,
}

crate::outside_table_catalog!(game_form, game_form_spell);
