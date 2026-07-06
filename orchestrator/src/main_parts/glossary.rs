fn apply_glossary_to_segments(
    storage: &Storage,
    segments: Vec<Segment>,
) -> Result<Vec<Segment>, String> {
    let entries = storage
        .list_glossary_entries()
        .map_err(|error| format!("Failed to load glossary: {error}"))?
        .iter()
        .map(glossary_entry_to_processor)
        .filter(|entry| !entry.aliases.is_empty())
        .collect::<Vec<_>>();

    if entries.is_empty() {
        return Ok(segments);
    }

    let processor = PostProcessor::new(entries);
    Ok(segments
        .into_iter()
        .map(|segment| apply_glossary_to_segment(&processor, segment))
        .collect())
}

fn glossary_entry_to_processor(entry: &GlossaryEntryRow) -> GlossaryEntry {
    GlossaryEntry {
        canonical: entry.canonical.clone(),
        aliases: entry
            .aliases
            .iter()
            .map(|alias| alias.alias.clone())
            .collect(),
        pinyin_forms: entry
            .aliases
            .iter()
            .filter_map(|alias| alias.pinyin.clone())
            .collect(),
        category: entry.category.clone(),
        enabled: entry.enabled,
    }
}

fn apply_glossary_to_segment(processor: &PostProcessor, mut segment: Segment) -> Segment {
    let corrected = processor.apply_to_segment(&segment);
    if corrected.corrected_text != segment.text {
        let mut corrections = corrected.auto_applied;
        corrections.extend(term_conflict_corrections(corrected.needs_confirmation));
        segment.corrections.extend(corrections);
        segment.text = corrected.corrected_text;
    } else if !corrected.needs_confirmation.is_empty()
        && !segment
            .low_confidence_reasons
            .iter()
            .any(|reason| reason == "term_conflict")
    {
        segment.low_confidence_reasons.push("term_conflict".into());
    }
    segment
}

fn term_conflict_corrections(corrections: Vec<Correction>) -> Vec<Correction> {
    corrections
        .into_iter()
        .map(|mut correction| {
            correction.auto_applied = false;
            correction
        })
        .collect()
}
