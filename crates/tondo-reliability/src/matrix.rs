//! Stable extraction and classification of normative Tondo 0.1 requirements.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tondo_conformance::lineage::{DRAFT_LINEAGE_PATH, DraftLineage};
use tondo_conformance::manifest::{ConformanceCase, LoadedSuite};

use crate::inventory::Inventory;
use crate::{canonical_json, sha256};

pub const FORMAT: &str = "tondo-normative-coverage/1";
pub const EVIDENCE_FORMAT: &str = "tondo-normative-evidence/1";
pub const EVIDENCE_PATH: &str = "testing/normative-evidence.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageMatrix {
    pub format: String,
    pub document: String,
    pub edition: String,
    pub document_sha256: String,
    pub inventory_sha256: String,
    pub target: String,
    pub summary: MatrixSummary,
    pub requirements: Vec<Requirement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatrixSummary {
    pub total: u64,
    pub by_status: BTreeMap<String, u64>,
    pub by_risk: BTreeMap<String, u64>,
    pub with_executable_evidence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirement {
    pub id: String,
    pub revision: String,
    pub heading: String,
    pub heading_anchor: String,
    pub line_start: u32,
    pub line_end: u32,
    pub text: String,
    pub text_sha256: String,
    pub phase: String,
    pub risk: String,
    pub status: String,
    pub classification_reason: String,
    pub evidence: Vec<String>,
    pub dimensions: Dimensions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dimensions {
    pub positive: Dimension,
    pub rejection_or_failure: Dimension,
    pub boundary: Dimension,
    pub composition: Dimension,
    pub oracle: Dimension,
    pub public_boundary: Dimension,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dimension {
    pub evidence: Vec<String>,
    pub waiver: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceMap {
    format: String,
    document: String,
    edition: String,
    document_sha256: String,
    target: String,
    claims: Vec<EvidenceClaim>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceClaim {
    requirements: Vec<String>,
    dimensions: Dimensions,
}

impl Dimension {
    fn evidence(values: impl IntoIterator<Item = String>) -> Self {
        let mut evidence = values.into_iter().collect::<Vec<_>>();
        evidence.sort();
        evidence.dedup();
        Self {
            evidence,
            waiver: None,
        }
    }

    fn waived(reason: impl Into<String>) -> Self {
        Self {
            evidence: Vec::new(),
            waiver: Some(reason.into()),
        }
    }
}

pub fn build(root: &Path, inventory: &Inventory) -> Result<CoverageMatrix, String> {
    let document_path = root.join("TONDO_LANGUAGE_SPEC.md");
    let document_bytes = fs::read(&document_path)
        .map_err(|error| format!("cannot read `{}`: {error}", document_path.display()))?;
    let document = std::str::from_utf8(&document_bytes)
        .map_err(|error| format!("TONDO_LANGUAGE_SPEC.md is not valid UTF-8: {error}"))?;
    let document_sha256 = sha256(&document_bytes);
    let inventory_sha256 = sha256(&canonical_json(inventory)?);
    let lineage = DraftLineage::load(root, Path::new(DRAFT_LINEAGE_PATH))
        .map_err(|error| error.to_string())?;
    let suite = lineage.baseline_suite();
    let baseline_document = std::str::from_utf8(lineage.baseline_specification())
        .map_err(|error| format!("baseline language specification is not valid UTF-8: {error}"))?;
    let baseline_requirements = extract_requirements(baseline_document)?
        .into_iter()
        .map(|requirement| (requirement.id, sha256(requirement.text.as_bytes())))
        .collect::<BTreeMap<_, _>>();
    let extracted = extract_requirements(document)?;
    let evidence = load_evidence(root, &document_sha256, inventory, &extracted)?;
    let implemented_requirements = lineage.implemented_requirements();
    let current_ids = extracted
        .iter()
        .map(|requirement| requirement.id.as_str())
        .collect::<BTreeSet<_>>();
    if let Some(requirement) = implemented_requirements
        .iter()
        .find(|requirement| !current_ids.contains(**requirement))
    {
        return Err(format!(
            "draft case layer names unknown requirement `{requirement}`"
        ));
    }
    let mut requirements = extracted
        .into_iter()
        .map(|item| {
            let claim = evidence.get(&item.id);
            let inherited_unchanged = baseline_requirements
                .get(&item.id)
                .is_some_and(|hash| *hash == sha256(item.text.as_bytes()));
            let implemented_draft = implemented_requirements.contains(item.id.as_str());
            classify(
                item,
                &document_sha256,
                suite,
                claim,
                inherited_unchanged,
                implemented_draft,
            )
        })
        .collect::<Vec<_>>();
    requirements.sort_by(|left, right| left.id.cmp(&right.id));
    require_unique_ids(&requirements)?;
    let summary = summarize(&requirements);
    let matrix = CoverageMatrix {
        format: FORMAT.into(),
        document: "TONDO_LANGUAGE_SPEC.md".into(),
        edition: "0.1".into(),
        document_sha256,
        inventory_sha256,
        target: "tondo-vm-hosted".into(),
        summary,
        requirements,
    };
    validate(&matrix)?;
    Ok(matrix)
}

pub fn validate(matrix: &CoverageMatrix) -> Result<(), String> {
    if matrix.format != FORMAT {
        return Err(format!(
            "unsupported coverage matrix format `{}`",
            matrix.format
        ));
    }
    if matrix.edition != "0.1" || matrix.target != "tondo-vm-hosted" {
        return Err("coverage matrix targets an unsupported edition or target".into());
    }
    if !is_sha256(&matrix.document_sha256) || !is_sha256(&matrix.inventory_sha256) {
        return Err("coverage matrix has an invalid source hash".into());
    }
    if matrix.requirements.is_empty() {
        return Err("coverage matrix contains no requirements".into());
    }
    require_unique_ids(&matrix.requirements)?;
    for requirement in &matrix.requirements {
        if requirement.id.is_empty()
            || requirement.revision.is_empty()
            || requirement.heading.is_empty()
            || requirement.heading_anchor.is_empty()
            || requirement.text.is_empty()
            || requirement.phase.is_empty()
            || requirement.risk.is_empty()
            || requirement.classification_reason.is_empty()
            || requirement.line_start == 0
            || requirement.line_end < requirement.line_start
            || !is_sha256(&requirement.text_sha256)
        {
            return Err(format!(
                "requirement `{}` contains incomplete metadata",
                requirement.id
            ));
        }
        if !matches!(
            requirement.status.as_str(),
            "covered"
                | "draft-pending"
                | "target-not-applicable"
                | "stdlib-pending"
                | "toolchain-limit"
        ) {
            return Err(format!(
                "requirement `{}` has unknown status `{}`",
                requirement.id, requirement.status
            ));
        }
        if !matches!(
            requirement.risk.as_str(),
            "critical" | "high" | "medium" | "low"
        ) {
            return Err(format!(
                "requirement `{}` has unknown risk `{}`",
                requirement.id, requirement.risk
            ));
        }
        require_sorted_unique(
            &format!("{} evidence", requirement.id),
            &requirement.evidence,
        )?;
        for (name, dimension) in [
            ("positive", &requirement.dimensions.positive),
            (
                "rejection-or-failure",
                &requirement.dimensions.rejection_or_failure,
            ),
            ("boundary", &requirement.dimensions.boundary),
            ("composition", &requirement.dimensions.composition),
            ("oracle", &requirement.dimensions.oracle),
            ("public-boundary", &requirement.dimensions.public_boundary),
        ] {
            validate_dimension(&requirement.id, name, dimension)?;
        }
        if requirement.status == "covered"
            && (requirement.evidence.is_empty()
                || requirement.dimensions.oracle.evidence.is_empty()
                || requirement.dimensions.public_boundary.evidence.is_empty())
        {
            return Err(format!(
                "covered requirement `{}` lacks executable evidence or an oracle",
                requirement.id
            ));
        }
    }
    if matrix.summary != summarize(&matrix.requirements) {
        return Err("coverage matrix summary does not match its requirements".into());
    }
    Ok(())
}

#[derive(Debug)]
struct ExtractedRequirement {
    id: String,
    heading: String,
    heading_anchor: String,
    line_start: u32,
    line_end: u32,
    text: String,
    section: String,
}

fn extract_requirements(document: &str) -> Result<Vec<ExtractedRequirement>, String> {
    let mut requirements = Vec::new();
    let mut heading = "Tondo: especificación del lenguaje".to_owned();
    let mut heading_anchor = "tondo-especificacion-del-lenguaje".to_owned();
    let mut section = "root".to_owned();
    let mut ordinal_by_heading = BTreeMap::<String, u32>::new();
    let mut in_fence = false;
    let mut paragraph = Vec::<(u32, String)>::new();

    let flush = |paragraph: &mut Vec<(u32, String)>,
                 requirements: &mut Vec<ExtractedRequirement>,
                 heading: &str,
                 heading_anchor: &str,
                 section: &str,
                 ordinal_by_heading: &mut BTreeMap<String, u32>|
     -> Result<(), String> {
        if paragraph.is_empty() {
            return Ok(());
        }
        let line_start = paragraph.first().expect("paragraph is not empty").0;
        let line_end = paragraph.last().expect("paragraph is not empty").0;
        let text = normalize_markdown_paragraph(paragraph);
        paragraph.clear();
        if text.starts_with("En este documento,") || !is_normative(&text) {
            return Ok(());
        }
        let ordinal = ordinal_by_heading
            .entry(heading_anchor.to_owned())
            .or_insert(0);
        *ordinal += 1;
        let stable_section = stable_component(section);
        requirements.push(ExtractedRequirement {
            id: format!("TL01-{stable_section}-R{ordinal:03}"),
            heading: heading.to_owned(),
            heading_anchor: heading_anchor.to_owned(),
            line_start,
            line_end,
            text,
            section: section.to_owned(),
        });
        Ok(())
    };

    for (index, line) in document.lines().enumerate() {
        let line_number = u32::try_from(index + 1)
            .map_err(|_| "language specification has too many lines".to_owned())?;
        if line.starts_with("~~~") {
            flush(
                &mut paragraph,
                &mut requirements,
                &heading,
                &heading_anchor,
                &section,
                &mut ordinal_by_heading,
            )?;
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if let Some(title) = line
            .strip_prefix("###### ")
            .or_else(|| line.strip_prefix("##### "))
            .or_else(|| line.strip_prefix("#### "))
            .or_else(|| line.strip_prefix("### "))
            .or_else(|| line.strip_prefix("## "))
            .or_else(|| line.strip_prefix("# "))
        {
            flush(
                &mut paragraph,
                &mut requirements,
                &heading,
                &heading_anchor,
                &section,
                &mut ordinal_by_heading,
            )?;
            heading = title.trim().to_owned();
            heading_anchor = markdown_anchor(title);
            section = heading_section(title);
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            flush(
                &mut paragraph,
                &mut requirements,
                &heading,
                &heading_anchor,
                &section,
                &mut ordinal_by_heading,
            )?;
            continue;
        }
        if (trimmed.starts_with("- ") || is_numbered_list_item(trimmed)) && !paragraph.is_empty() {
            flush(
                &mut paragraph,
                &mut requirements,
                &heading,
                &heading_anchor,
                &section,
                &mut ordinal_by_heading,
            )?;
        }
        paragraph.push((line_number, trimmed.to_owned()));
    }
    flush(
        &mut paragraph,
        &mut requirements,
        &heading,
        &heading_anchor,
        &section,
        &mut ordinal_by_heading,
    )?;
    if in_fence {
        return Err("TONDO_LANGUAGE_SPEC.md contains an unclosed fence".into());
    }
    Ok(requirements)
}

fn classify(
    extracted: ExtractedRequirement,
    document_sha256: &str,
    suite: &LoadedSuite,
    claim: Option<&EvidenceClaim>,
    inherited_unchanged: bool,
    implemented_draft: bool,
) -> Requirement {
    let codes = diagnostic_codes(&extracted.text);
    let failure_cases = matching_cases(&suite.manifest().cases, &codes, false);
    let positive_cases = matching_cases(&suite.manifest().cases, &codes, true);
    let covered_codes = suite
        .manifest()
        .cases
        .iter()
        .flat_map(|case| case.covers.iter().cloned())
        .collect::<BTreeSet<_>>();
    let location = format!(
        "TONDO_LANGUAGE_SPEC.md:{}#{}",
        extracted.line_start, extracted.heading_anchor
    );
    let (status, reason, evidence, dimensions) = if !inherited_unchanged {
        if !implemented_draft {
            let reason = "The requirement is new or changed since the bootstrap baseline and no draft case layer claims its implementation.";
            (
                "draft-pending",
                reason,
                vec![location.clone()],
                waived_dimensions(reason),
            )
        } else if let Some(claim) = claim {
            (
                "covered",
                "A draft case layer and the versioned normative evidence map link this requirement to executable public-boundary evidence.",
                claim_evidence(claim),
                claim.dimensions.clone(),
            )
        } else {
            let reason = "A draft case layer names this new or changed requirement, but no reviewed executable evidence claim covers it.";
            (
                "toolchain-limit",
                reason,
                vec![location.clone()],
                waived_dimensions(reason),
            )
        }
    } else if let Some(claim) = claim {
        (
            "covered",
            "The versioned normative evidence map links this requirement to executable public-boundary evidence.",
            claim_evidence(claim),
            claim.dimensions.clone(),
        )
    } else if target_not_applicable(&extracted.section, &extracted.text) {
        let reason =
            "The requirement describes a deliberately absent or non-bootstrap target surface.";
        (
            "target-not-applicable",
            reason,
            vec![location.clone()],
            waived_dimensions(reason),
        )
    } else if stdlib_pending(&extracted.section, &extracted.text) {
        let reason = "The requirement belongs to the standard-library contract, which is outside Tondo 0.1 language conformance.";
        (
            "stdlib-pending",
            reason,
            vec![location.clone()],
            waived_dimensions(reason),
        )
    } else if complete_code_coverage(&codes, &covered_codes) {
        let mut evidence = failure_cases.clone();
        evidence.extend(positive_cases.iter().cloned());
        evidence.sort();
        evidence.dedup();
        let positive = if positive_cases.is_empty() {
            Dimension::waived(
                "The normative rule is itself a required failure and has no distinct positive dimension.",
            )
        } else {
            Dimension::evidence(positive_cases.clone())
        };
        (
            "covered",
            "The normative text names a stable diagnostic or panic with executable conformance evidence.",
            evidence,
            Dimensions {
                positive,
                rejection_or_failure: Dimension::evidence(failure_cases.clone()),
                boundary: Dimension::evidence(failure_cases.clone()),
                composition: Dimension::evidence(failure_cases.clone()),
                oracle: Dimension::evidence(["conformance:exact-observation".into()]),
                public_boundary: Dimension::evidence(["conformance:adapter-protocol".into()]),
            },
        )
    } else {
        let reason = if codes.is_empty() {
            "No case currently carries this prose requirement as an explicit stable identity; section-level examples are not counted as semantic coverage."
        } else {
            "The named diagnostic has no complete executable evidence set linked to this prose requirement."
        };
        (
            "toolchain-limit",
            reason,
            vec![location.clone()],
            waived_dimensions(reason),
        )
    };
    Requirement {
        id: extracted.id,
        revision: format!("0.1@{}", &document_sha256[..16]),
        heading: extracted.heading,
        heading_anchor: extracted.heading_anchor,
        line_start: extracted.line_start,
        line_end: extracted.line_end,
        text_sha256: sha256(extracted.text.as_bytes()),
        text: extracted.text,
        phase: phase_for_section(&extracted.section).into(),
        risk: risk_for_section(&extracted.section).into(),
        status: status.into(),
        classification_reason: reason.into(),
        evidence,
        dimensions,
    }
}

fn load_evidence(
    root: &Path,
    document_sha256: &str,
    inventory: &Inventory,
    requirements: &[ExtractedRequirement],
) -> Result<BTreeMap<String, EvidenceClaim>, String> {
    let path = root.join(EVIDENCE_PATH);
    let bytes =
        fs::read(&path).map_err(|error| format!("cannot read `{}`: {error}", path.display()))?;
    let evidence: EvidenceMap = serde_json::from_slice(&bytes)
        .map_err(|error| format!("cannot decode `{}`: {error}", path.display()))?;
    validate_evidence(
        &evidence,
        document_sha256,
        inventory,
        &requirements
            .iter()
            .map(|requirement| requirement.id.as_str())
            .collect::<BTreeSet<_>>(),
    )
}

fn validate_evidence(
    evidence: &EvidenceMap,
    document_sha256: &str,
    inventory: &Inventory,
    requirements: &BTreeSet<&str>,
) -> Result<BTreeMap<String, EvidenceClaim>, String> {
    if evidence.format != EVIDENCE_FORMAT
        || evidence.document != "TONDO_LANGUAGE_SPEC.md"
        || evidence.edition != "0.1"
        || evidence.target != "tondo-vm-hosted"
    {
        return Err("normative evidence targets an unsupported format, document, or target".into());
    }
    if evidence.document_sha256 != document_sha256 {
        return Err("normative evidence does not match the current language specification".into());
    }

    let inventory = inventory
        .tests
        .iter()
        .map(|test| (test.id.as_str(), test))
        .collect::<BTreeMap<_, _>>();
    let mut result = BTreeMap::new();
    let mut previous = None::<&str>;
    for claim in &evidence.claims {
        require_sorted_unique("normative evidence requirements", &claim.requirements)?;
        if claim.requirements.is_empty() {
            return Err("normative evidence claims require requirements".into());
        }
        for (name, dimension) in claim_dimensions(&claim.dimensions) {
            validate_dimension("normative evidence", name, dimension)?;
        }
        for (name, dimension) in claim_dimensions(&claim.dimensions) {
            for test_id in &dimension.evidence {
                let test = inventory
                    .get(test_id.as_str())
                    .ok_or_else(|| format!("normative evidence names unknown test `{test_id}`"))?;
                if test.status != "executable" {
                    return Err(format!(
                        "normative evidence test `{test_id}` is not executable"
                    ));
                }
                if name == "public-boundary"
                    && (test.target != evidence.target
                        || !matches!(
                            test.kind.as_str(),
                            "conformance-case" | "fixture" | "spec-fence"
                        ))
                {
                    return Err(format!(
                        "normative evidence test `{test_id}` is not a public-boundary test"
                    ));
                }
            }
        }
        if claim.dimensions.oracle.evidence.is_empty()
            || claim.dimensions.public_boundary.evidence.is_empty()
        {
            return Err(
                "normative evidence requires executable oracle and public-boundary dimensions"
                    .into(),
            );
        }

        for requirement in &claim.requirements {
            if !requirements.contains(requirement.as_str()) {
                return Err(format!(
                    "normative evidence names unknown requirement `{requirement}`"
                ));
            }
            if previous.is_some_and(|previous| previous >= requirement.as_str()) {
                return Err("normative evidence claims must be globally sorted and unique".into());
            }
            previous = Some(requirement);
            result.insert(requirement.clone(), claim.clone());
        }
    }
    Ok(result)
}

fn claim_dimensions(dimensions: &Dimensions) -> [(&'static str, &Dimension); 6] {
    [
        ("positive", &dimensions.positive),
        ("rejection-or-failure", &dimensions.rejection_or_failure),
        ("boundary", &dimensions.boundary),
        ("composition", &dimensions.composition),
        ("oracle", &dimensions.oracle),
        ("public-boundary", &dimensions.public_boundary),
    ]
}

fn claim_evidence(claim: &EvidenceClaim) -> Vec<String> {
    let mut evidence = claim_dimensions(&claim.dimensions)
        .into_iter()
        .flat_map(|(_, dimension)| dimension.evidence.iter().cloned())
        .collect::<Vec<_>>();
    evidence.sort();
    evidence.dedup();
    evidence
}

#[cfg(test)]
fn evidence_dimensions(test: &str) -> Dimensions {
    let dimension = || Dimension::evidence([test.to_owned()]);
    Dimensions {
        positive: dimension(),
        rejection_or_failure: dimension(),
        boundary: dimension(),
        composition: dimension(),
        oracle: dimension(),
        public_boundary: dimension(),
    }
}

fn matching_cases(cases: &[ConformanceCase], codes: &[String], positive: bool) -> Vec<String> {
    let mut result = cases
        .iter()
        .filter(|case| {
            let values = if positive {
                &case.positive_for
            } else {
                &case.covers
            };
            codes.iter().any(|code| values.contains(code))
        })
        .map(|case| format!("conformance:{}", case.id))
        .collect::<Vec<_>>();
    result.sort();
    result.dedup();
    result
}

fn complete_code_coverage(codes: &[String], covered_codes: &BTreeSet<String>) -> bool {
    !codes.is_empty() && codes.iter().all(|code| covered_codes.contains(code))
}

fn waived_dimensions(reason: &str) -> Dimensions {
    Dimensions {
        positive: Dimension::waived(reason),
        rejection_or_failure: Dimension::waived(reason),
        boundary: Dimension::waived(reason),
        composition: Dimension::waived(reason),
        oracle: Dimension::waived(reason),
        public_boundary: Dimension::waived(reason),
    }
}

fn validate_dimension(id: &str, name: &str, dimension: &Dimension) -> Result<(), String> {
    require_sorted_unique(&format!("{id} {name} evidence"), &dimension.evidence)?;
    match (dimension.evidence.is_empty(), dimension.waiver.as_deref()) {
        (false, None) => Ok(()),
        (true, Some(reason)) if !reason.trim().is_empty() => Ok(()),
        (false, Some(_)) => Err(format!(
            "requirement `{id}` dimension `{name}` cannot have evidence and a waiver"
        )),
        (true, _) => Err(format!(
            "requirement `{id}` dimension `{name}` needs evidence or a versioned waiver"
        )),
    }
}

fn summarize(requirements: &[Requirement]) -> MatrixSummary {
    let mut by_status = BTreeMap::new();
    let mut by_risk = BTreeMap::new();
    let mut with_executable_evidence = 0;
    for requirement in requirements {
        *by_status.entry(requirement.status.clone()).or_insert(0) += 1;
        *by_risk.entry(requirement.risk.clone()).or_insert(0) += 1;
        if requirement.status == "covered" {
            with_executable_evidence += 1;
        }
    }
    MatrixSummary {
        total: requirements.len() as u64,
        by_status,
        by_risk,
        with_executable_evidence,
    }
}

fn normalize_markdown_paragraph(lines: &[(u32, String)]) -> String {
    lines
        .iter()
        .map(|(_, line)| {
            line.trim_start_matches("- ")
                .trim_start_matches(|character: char| character.is_ascii_digit())
                .trim_start_matches(". ")
                .trim()
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_normative(text: &str) -> bool {
    let lower = text.to_lowercase();
    [
        " debe ",
        " debe.",
        " debe:",
        " deben ",
        " deben.",
        " deberá ",
        " deberán ",
        " no puede ",
        " no puede.",
        " no pueden ",
    ]
    .iter()
    .any(|needle| format!(" {lower} ").contains(needle))
}

fn diagnostic_codes(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut result = Vec::new();
    for index in 0..bytes.len().saturating_sub(4) {
        if !matches!(bytes[index], b'E' | b'W' | b'P')
            || !bytes[index + 1..index + 5].iter().all(u8::is_ascii_digit)
        {
            continue;
        }
        if bytes
            .get(index.wrapping_sub(1))
            .is_some_and(u8::is_ascii_alphanumeric)
            || bytes.get(index + 5).is_some_and(u8::is_ascii_alphanumeric)
        {
            continue;
        }
        result.push(text[index..index + 5].to_owned());
    }
    result.sort();
    result.dedup();
    result
}

fn markdown_anchor(title: &str) -> String {
    let mut anchor = String::new();
    let mut pending_dash = false;
    for character in title.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() || character == '_' {
            if pending_dash && !anchor.is_empty() {
                anchor.push('-');
            }
            anchor.push(character);
            pending_dash = false;
        } else {
            pending_dash = true;
        }
    }
    anchor
}

fn heading_section(title: &str) -> String {
    let first = title
        .split_ascii_whitespace()
        .next()
        .unwrap_or("root")
        .trim_end_matches('.');
    if first
        .chars()
        .next()
        .is_some_and(|item| item.is_ascii_digit())
    {
        first.into()
    } else if title.starts_with("Apéndice A") {
        "appendix-a".into()
    } else if title.starts_with("Apéndice B") {
        "appendix-b".into()
    } else if title.starts_with("Apéndice C") {
        "appendix-c".into()
    } else {
        stable_component(title)
    }
}

fn stable_component(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            output.push(character.to_ascii_uppercase());
        } else if matches!(character, '.' | '-' | '_' | ' ') && !output.ends_with('-') {
            output.push('-');
        }
    }
    output.trim_matches('-').to_owned()
}

fn is_numbered_list_item(line: &str) -> bool {
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    digits > 0 && line.as_bytes().get(digits..digits + 2) == Some(b". ")
}

fn top_level_section(section: &str) -> Option<u32> {
    section.split('.').next()?.parse().ok()
}

fn target_not_applicable(section: &str, text: &str) -> bool {
    let lower = text.to_lowercase();
    top_level_section(section) == Some(25)
        || lower.contains("frontera nativa")
        || lower.contains("especificación ffi")
        || lower.contains("backend nativo")
        || lower.contains("una futura edición")
}

fn stdlib_pending(section: &str, text: &str) -> bool {
    top_level_section(section) == Some(26)
        || text.to_lowercase().contains("futura librería estándar")
}

fn phase_for_section(section: &str) -> &'static str {
    match top_level_section(section) {
        Some(5) => "frontend",
        Some(6 | 7) => "resolution",
        Some(8..=12) => "types-hir",
        Some(13..=15) => "control-errors",
        Some(16) => "ownership-runtime",
        Some(17..=19) => "operators-values",
        Some(20) => "runtime-host",
        Some(21) => "formatter-documentation",
        Some(22) => "diagnostics-tooling",
        Some(23) => "grammar",
        Some(24) => "integrated-examples",
        Some(25 | 26) => "boundary",
        Some(27) => "metaprogramming",
        Some(28) => "testing",
        _ => "language-contract",
    }
}

fn risk_for_section(section: &str) -> &'static str {
    match top_level_section(section) {
        Some(5 | 6 | 7 | 8 | 12 | 13 | 14 | 15 | 16 | 20 | 22) => "critical",
        Some(9 | 10 | 11 | 17 | 18 | 19 | 23) => "high",
        Some(21 | 24 | 26) => "medium",
        Some(27 | 28) => "critical",
        _ => "low",
    }
}

fn require_unique_ids(requirements: &[Requirement]) -> Result<(), String> {
    let mut ids = BTreeSet::new();
    for requirement in requirements {
        if !ids.insert(requirement.id.as_str()) {
            return Err(format!(
                "duplicate coverage requirement ID `{}`",
                requirement.id
            ));
        }
    }
    Ok(())
}

fn require_sorted_unique(context: &str, values: &[String]) -> Result<(), String> {
    if values.windows(2).all(|pair| pair[0] < pair[1]) {
        Ok(())
    } else {
        Err(format!("{context} must be sorted and unique"))
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use crate::inventory::{DocumentRevision, InventorySummary, TestEntry};

    use super::*;

    #[test]
    fn extraction_ignores_code_and_assigns_stable_heading_ordinals() {
        let document = "\
# Spec

## 1. Core

El compilador debe aceptar el caso.

~~~tondo
// no puede count
~~~

- El valor no puede escapar.
";
        let requirements = extract_requirements(document).unwrap();
        assert_eq!(requirements.len(), 2);
        assert_eq!(requirements[0].id, "TL01-1-R001");
        assert_eq!(requirements[1].id, "TL01-1-R002");
        assert_eq!(requirements[0].line_start, 5);
        assert_eq!(requirements[1].line_start, 11);
    }

    #[test]
    fn diagnostic_extraction_is_closed_and_deduplicated() {
        assert_eq!(
            diagnostic_codes("`E0001`, E0001, P0011 and W1002; not XE0001"),
            ["E0001", "P0011", "W1002"]
        );
        let covered = ["E0001".to_owned()].into_iter().collect();
        assert!(complete_code_coverage(&["E0001".into()], &covered));
        assert!(!complete_code_coverage(
            &["E0001".into(), "P0011".into()],
            &covered
        ));
        assert!(!complete_code_coverage(&[], &covered));
    }

    #[test]
    fn dimensions_require_evidence_xor_a_waiver() {
        assert!(
            validate_dimension("REQ", "positive", &Dimension::evidence(["case".into()])).is_ok()
        );
        assert!(
            validate_dimension("REQ", "positive", &Dimension::waived("not applicable")).is_ok()
        );
        assert!(
            validate_dimension(
                "REQ",
                "positive",
                &Dimension {
                    evidence: vec!["case".into()],
                    waiver: Some("both".into()),
                },
            )
            .is_err()
        );
    }

    #[test]
    fn changed_draft_requirements_cannot_inherit_baseline_evidence() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap();
        let lineage = DraftLineage::load(root, DRAFT_LINEAGE_PATH).unwrap();
        let extracted = || ExtractedRequirement {
            id: "TL01-LIVE-R001".into(),
            heading: "Draft requirement".into(),
            heading_anchor: "draft-requirement".into(),
            line_start: 1,
            line_end: 1,
            text: "El compilador debe aceptar la forma viva.".into(),
            section: "28".into(),
        };
        let document_sha256 = "a".repeat(64);

        let pending = classify(
            extracted(),
            &document_sha256,
            lineage.baseline_suite(),
            None,
            false,
            false,
        );
        assert_eq!(pending.status, "draft-pending");

        let declared_without_evidence = classify(
            extracted(),
            &document_sha256,
            lineage.baseline_suite(),
            None,
            false,
            true,
        );
        assert_eq!(declared_without_evidence.status, "toolchain-limit");
        assert!(
            declared_without_evidence
                .classification_reason
                .contains("no reviewed executable evidence")
        );

        let claim = EvidenceClaim {
            requirements: vec!["TL01-LIVE-R001".into()],
            dimensions: evidence_dimensions("conformance:case"),
        };
        let covered = classify(
            extracted(),
            &document_sha256,
            lineage.baseline_suite(),
            Some(&claim),
            false,
            true,
        );
        assert_eq!(covered.status, "covered");
        assert_eq!(covered.evidence, ["conformance:case"]);
    }

    #[test]
    fn normative_evidence_is_closed_and_requires_a_real_public_test() {
        let document_sha256 = "a".repeat(64);
        let requirement = "TL01-1-R001";
        let requirements = [requirement].into_iter().collect::<BTreeSet<_>>();
        let inventory = evidence_inventory("executable", "tondo-vm-hosted", "conformance-case");
        let mut evidence = EvidenceMap {
            format: EVIDENCE_FORMAT.into(),
            document: "TONDO_LANGUAGE_SPEC.md".into(),
            edition: "0.1".into(),
            document_sha256: document_sha256.clone(),
            target: "tondo-vm-hosted".into(),
            claims: vec![EvidenceClaim {
                requirements: vec![requirement.into()],
                dimensions: evidence_dimensions("conformance:case"),
            }],
        };

        let validated =
            validate_evidence(&evidence, &document_sha256, &inventory, &requirements).unwrap();
        assert_eq!(
            validated.keys().map(String::as_str).collect::<Vec<_>>(),
            [requirement]
        );
        let claim = validated.get(requirement).unwrap();
        assert_eq!(
            claim.dimensions.public_boundary.evidence,
            ["conformance:case"]
        );
        assert_eq!(claim_evidence(claim), ["conformance:case"]);

        evidence.document_sha256 = "b".repeat(64);
        assert!(
            validate_evidence(&evidence, &document_sha256, &inventory, &requirements)
                .unwrap_err()
                .contains("does not match")
        );
        evidence.document_sha256 = document_sha256.clone();

        evidence.claims[0].requirements = vec!["TL01-UNKNOWN".into()];
        assert!(
            validate_evidence(&evidence, &document_sha256, &inventory, &requirements)
                .unwrap_err()
                .contains("unknown requirement")
        );
        evidence.claims[0].requirements = vec![requirement.into()];

        evidence.claims[0].dimensions.positive.evidence = vec!["conformance:missing".into()];
        assert!(
            validate_evidence(&evidence, &document_sha256, &inventory, &requirements)
                .unwrap_err()
                .contains("unknown test")
        );
        evidence.claims[0].dimensions.positive.evidence = vec!["conformance:case".into()];

        evidence.claims[0].dimensions.boundary.evidence.clear();
        assert!(
            validate_evidence(&evidence, &document_sha256, &inventory, &requirements)
                .unwrap_err()
                .contains("needs evidence")
        );
        evidence.claims[0].dimensions.boundary.waiver =
            Some("The rule has no material boundary.".into());
        validate_evidence(&evidence, &document_sha256, &inventory, &requirements).unwrap();

        let internal = evidence_inventory("executable", "host-rust", "rust-test");
        assert!(
            validate_evidence(&evidence, &document_sha256, &internal, &requirements)
                .unwrap_err()
                .contains("public-boundary")
        );
        let future = evidence_inventory("future-contract", "tondo-vm-hosted", "conformance-case");
        assert!(
            validate_evidence(&evidence, &document_sha256, &future, &requirements)
                .unwrap_err()
                .contains("not executable")
        );
    }

    fn evidence_inventory(status: &str, target: &str, kind: &str) -> Inventory {
        Inventory {
            format: crate::inventory::FORMAT.into(),
            repository: "tonyredondo/tondo".into(),
            documents: vec![DocumentRevision {
                path: "TONDO_LANGUAGE_SPEC.md".into(),
                edition: "0.1".into(),
                status: "normative".into(),
                sha256: "a".repeat(64),
            }],
            summary: InventorySummary {
                logical_tests: 1,
                repetitions: 1,
                physical_sources: 1,
                unique_source_hashes: 1,
                by_kind: BTreeMap::new(),
                by_status: BTreeMap::new(),
                by_phase: BTreeMap::new(),
            },
            tests: vec![TestEntry {
                id: "conformance:case".into(),
                kind: kind.into(),
                crate_name: Some("tondo-conformance".into()),
                phase: "runtime".into(),
                source: "case.expect.json".into(),
                fixture: Some("case".into()),
                group: "runtime".into(),
                requirements: Vec::new(),
                oracle: "exact-observation".into(),
                repetitions: 1,
                source_sha256: "b".repeat(64),
                target: target.into(),
                document: Some("TONDO_LANGUAGE_SPEC.md".into()),
                edition: "0.1".into(),
                status: status.into(),
                sidecars: Vec::new(),
            }],
        }
    }
}
