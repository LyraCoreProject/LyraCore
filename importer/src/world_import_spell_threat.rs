use super::*;

/// vmangos `SpellMgr::LoadSpellThreats`. It reads `threat` as a uint16, so a fraction truncates.
pub(super) fn rows(source: &impl TableRows, known: impl Fn(u32) -> bool) -> Result<Vec<String>> {
    let mut rows = Vec::new();
    for row in source.rows("spell_threat")?.iter() {
        let id: u32 = row.number("entry")?;
        let threat: f32 = row.number("threat")?;
        let multiplier: f32 = row.number("multiplier")?;
        let inverse_effect_mask: u8 = row.number("inverse_effect_mask")?;
        ensure!(id != 0 && known(id), "spell_threat {id}: missing spell");
        ensure!(
            (0.0..=f32::from(u16::MAX)).contains(&threat)
                && multiplier.is_finite()
                && multiplier >= 0.0,
            "spell_threat {id}: invalid threat or multiplier"
        );
        ensure!(
            inverse_effect_mask <= 0b111,
            "spell_threat {id}: inverse_effect_mask names no effect index"
        );
        rows.push(format!(
            "({id},{},{multiplier},{inverse_effect_mask})",
            threat as u16
        ));
    }
    Ok(rows)
}
