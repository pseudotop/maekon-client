//! Explicit local input revisions and fixed model provenance (#12140).
//! These bounds do not authenticate a document or verify a hash preimage. The
//! native registration store must do that before exposing a query to a provider.

use std::fmt;

use super::{valid_id, valid_text, WbsCandidateError};

/// Stable registration identity and exact input revisions. No path, organization
/// token or server approval is invented for a standalone document.
#[derive(Clone, PartialEq, Eq)]
pub struct WbsLocalInputRevision {
    document_registration_id: String,
    input_revision: String,
    input_hash: String,
    roster_revision: String,
    roster_hash: String,
}
impl WbsLocalInputRevision {
    pub fn new(
        document_registration_id: String,
        input_revision: String,
        input_hash: String,
        roster_revision: String,
        roster_hash: String,
    ) -> Result<Self, WbsCandidateError> {
        if !valid_id(&document_registration_id)
            || !valid_id(&input_revision)
            || !valid_id(&roster_revision)
            || !local_hash(&input_hash)
            || !local_hash(&roster_hash)
        {
            return Err(WbsCandidateError::InvalidInput);
        }
        Ok(Self {
            document_registration_id,
            input_revision,
            input_hash,
            roster_revision,
            roster_hash,
        })
    }
    pub fn document_registration_id(&self) -> &str {
        &self.document_registration_id
    }
    pub fn input_revision(&self) -> &str {
        &self.input_revision
    }
    pub fn input_hash(&self) -> &str {
        &self.input_hash
    }
    pub fn roster_revision(&self) -> &str {
        &self.roster_revision
    }
    pub fn roster_hash(&self) -> &str {
        &self.roster_hash
    }
    pub fn ensure_current(&self, current: &Self) -> Result<(), WbsCandidateError> {
        if self != current {
            return Err(WbsCandidateError::Stale);
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct WbsLocalModelProvenance {
    provider_selection_digest: String,
    provider_name: String,
    model: String,
    prompt_revision: String,
}
impl WbsLocalModelProvenance {
    pub fn new(
        provider_selection_digest: String,
        provider_name: String,
        model: String,
        prompt_revision: String,
    ) -> Result<Self, WbsCandidateError> {
        if !local_hash(&provider_selection_digest)
            || !valid_id(&provider_name)
            || !valid_text(&model, 128, false)
            || model.trim() != model
            || !valid_id(&prompt_revision)
        {
            return Err(WbsCandidateError::InvalidInput);
        }
        Ok(Self {
            provider_selection_digest,
            provider_name,
            model,
            prompt_revision,
        })
    }
    pub fn provider_selection_digest(&self) -> &str {
        &self.provider_selection_digest
    }
    pub fn provider_name(&self) -> &str {
        &self.provider_name
    }
    pub fn model(&self) -> &str {
        &self.model
    }
    pub fn prompt_revision(&self) -> &str {
        &self.prompt_revision
    }
}

fn local_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
macro_rules! redacted_debug {
    ($($ty:ty),+ $(,)?) => { $(
        impl fmt::Debug for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($ty), "([REDACTED])"))
            }
        }
    )+ };
}
redacted_debug!(WbsLocalInputRevision, WbsLocalModelProvenance);

use std::collections::HashSet;

pub const MAX_LOCAL_EVIDENCE: usize = 8;

#[derive(Clone, PartialEq, Eq)]
pub struct WbsLocalTask {
    wbs_item_id: String,
    task_text: String,
}
impl WbsLocalTask {
    pub fn new(wbs_item_id: String, task_text: String) -> Result<Self, WbsCandidateError> {
        if !valid_id(&wbs_item_id) || !explicit_text(&task_text, 4096) {
            return Err(WbsCandidateError::InvalidInput);
        }
        Ok(Self {
            wbs_item_id,
            task_text,
        })
    }
    pub fn wbs_item_id(&self) -> &str {
        &self.wbs_item_id
    }
    pub fn task_text(&self) -> &str {
        &self.task_text
    }
}

/// Explicit facts supplied with a roster member; not model-invented experience.
#[derive(Clone, PartialEq, Eq)]
pub struct WbsLocalEvidence {
    evidence_id: String,
    description: String,
}
impl WbsLocalEvidence {
    pub fn new(evidence_id: String, description: String) -> Result<Self, WbsCandidateError> {
        if !valid_id(&evidence_id) || !explicit_text(&description, 512) {
            return Err(WbsCandidateError::InvalidInput);
        }
        Ok(Self {
            evidence_id,
            description,
        })
    }
    pub fn evidence_id(&self) -> &str {
        &self.evidence_id
    }
    pub fn description(&self) -> &str {
        &self.description
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct WbsLocalRosterMember {
    person_id: String,
    display_name: String,
    evidence: Vec<WbsLocalEvidence>,
}
impl WbsLocalRosterMember {
    pub fn new(
        person_id: String,
        display_name: String,
        evidence: Vec<WbsLocalEvidence>,
    ) -> Result<Self, WbsCandidateError> {
        let ids: HashSet<_> = evidence.iter().map(WbsLocalEvidence::evidence_id).collect();
        if !valid_id(&person_id)
            || !safe_assignee(&display_name)
            || evidence.len() > MAX_LOCAL_EVIDENCE
            || ids.len() != evidence.len()
        {
            return Err(WbsCandidateError::InvalidInput);
        }
        Ok(Self {
            person_id,
            display_name,
            evidence,
        })
    }
    pub fn person_id(&self) -> &str {
        &self.person_id
    }
    pub fn display_name(&self) -> &str {
        &self.display_name
    }
    pub fn evidence(&self) -> &[WbsLocalEvidence] {
        &self.evidence
    }
}

fn explicit_text(value: &str, max_bytes: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= max_bytes
        && !value
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
}
fn safe_assignee(value: &str) -> bool {
    valid_text(value, 256, false)
        && value.trim() == value
        && !value.starts_with(['=', '+', '-', '@', '\''])
}

redacted_debug!(WbsLocalTask, WbsLocalEvidence, WbsLocalRosterMember);

use super::WbsCandidateSnapshotRef;

pub const WBS_LOCAL_CONTRACT_VERSION: &str = "local-document-roster.v1";
pub const MAX_LOCAL_TASKS: usize = 64;
pub const MAX_LOCAL_ROSTER_MEMBERS: usize = 32;
pub const MAX_LOCAL_INPUT_TEXT_BYTES: usize = 65_536;

/// An immutable explicit input. Empty roster is valid and grants no permission
/// to invent people. Native registration and provider verify revision hashes.
#[derive(Clone, PartialEq, Eq)]
pub struct WbsLocalDocumentInput {
    revision: WbsLocalInputRevision,
    tasks: Vec<WbsLocalTask>,
    roster: Vec<WbsLocalRosterMember>,
    synthetic: bool,
}
impl WbsLocalDocumentInput {
    pub fn new(
        revision: WbsLocalInputRevision,
        tasks: Vec<WbsLocalTask>,
        roster: Vec<WbsLocalRosterMember>,
        synthetic: bool,
    ) -> Result<Self, WbsCandidateError> {
        let task_ids: HashSet<_> = tasks.iter().map(WbsLocalTask::wbs_item_id).collect();
        let person_ids: HashSet<_> = roster.iter().map(WbsLocalRosterMember::person_id).collect();
        let text_bytes: usize = tasks.iter().map(|task| task.task_text.len()).sum::<usize>()
            + roster
                .iter()
                .map(|member| {
                    member.display_name.len()
                        + member
                            .evidence
                            .iter()
                            .map(|fact| fact.description.len())
                            .sum::<usize>()
                })
                .sum::<usize>();
        if tasks.is_empty()
            || tasks.len() > MAX_LOCAL_TASKS
            || roster.len() > MAX_LOCAL_ROSTER_MEMBERS
            || task_ids.len() != tasks.len()
            || person_ids.len() != roster.len()
            || text_bytes > MAX_LOCAL_INPUT_TEXT_BYTES
        {
            return Err(WbsCandidateError::InvalidInput);
        }
        Ok(Self {
            revision,
            tasks,
            roster,
            synthetic,
        })
    }
    pub fn revision(&self) -> &WbsLocalInputRevision {
        &self.revision
    }
    pub fn tasks(&self) -> &[WbsLocalTask] {
        &self.tasks
    }
    pub fn roster(&self) -> &[WbsLocalRosterMember] {
        &self.roster
    }
    pub fn synthetic(&self) -> bool {
        self.synthetic
    }
    pub fn task(&self, item: &str) -> Option<&WbsLocalTask> {
        self.tasks.iter().find(|task| task.wbs_item_id == item)
    }
    pub fn member(&self, person_id: &str) -> Option<&WbsLocalRosterMember> {
        self.roster
            .iter()
            .find(|member| member.person_id == person_id)
    }
}

/// Reference identifies the already validated, fixed result. Refresh makes a new
/// query generation; selection and execution revalidate without another model call.
#[derive(Clone, PartialEq, Eq)]
pub struct WbsLocalResultSnapshot {
    reference: WbsCandidateSnapshotRef,
    input: WbsLocalInputRevision,
    model: WbsLocalModelProvenance,
}
impl WbsLocalResultSnapshot {
    pub fn new(
        reference: WbsCandidateSnapshotRef,
        input: WbsLocalInputRevision,
        model: WbsLocalModelProvenance,
    ) -> Result<Self, WbsCandidateError> {
        if !local_hash(reference.snapshot_hash()) {
            return Err(WbsCandidateError::InvalidResponse);
        }
        Ok(Self {
            reference,
            input,
            model,
        })
    }
    pub fn reference(&self) -> &WbsCandidateSnapshotRef {
        &self.reference
    }
    pub fn input(&self) -> &WbsLocalInputRevision {
        &self.input
    }
    pub fn model(&self) -> &WbsLocalModelProvenance {
        &self.model
    }
}

redacted_debug!(WbsLocalDocumentInput, WbsLocalResultSnapshot);

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn revision() -> Result<WbsLocalInputRevision, WbsCandidateError> {
        WbsLocalInputRevision::new(
            "document-private".into(),
            "input-v1".into(),
            "a".repeat(64),
            "roster-v1".into(),
            "b".repeat(64),
        )
    }

    #[test]
    fn local_hashes_require_exactly_64_lowercase_hexadecimal_bytes() {
        assert!(local_hash(&"09af".repeat(16)));
        for value in [
            String::new(),
            "a".repeat(63),
            "a".repeat(65),
            "A".repeat(64),
            "g".repeat(64),
            format!("{}g", "a".repeat(63)),
        ] {
            assert!(!local_hash(&value));
        }
    }

    #[test]
    fn revision_preserves_each_pin_and_rejects_each_invalid_dimension() -> TestResult {
        let revision = revision()?;
        assert_eq!(revision.document_registration_id(), "document-private");
        assert_eq!(revision.input_revision(), "input-v1");
        assert_eq!(revision.input_hash(), "a".repeat(64));
        assert_eq!(revision.roster_revision(), "roster-v1");
        assert_eq!(revision.roster_hash(), "b".repeat(64));
        let invalid = [
            ("", "input-v1", "a".repeat(64), "roster-v1", "b".repeat(64)),
            ("document", "", "a".repeat(64), "roster-v1", "b".repeat(64)),
            (
                "document",
                "input-v1",
                "A".repeat(64),
                "roster-v1",
                "b".repeat(64),
            ),
            ("document", "input-v1", "a".repeat(64), "", "b".repeat(64)),
            (
                "document",
                "input-v1",
                "a".repeat(64),
                "roster-v1",
                "b".repeat(63),
            ),
        ];
        for (document, input_revision, input_hash, roster_revision, roster_hash) in invalid {
            assert_eq!(
                WbsLocalInputRevision::new(
                    document.into(),
                    input_revision.into(),
                    input_hash,
                    roster_revision.into(),
                    roster_hash,
                ),
                Err(WbsCandidateError::InvalidInput)
            );
        }
        Ok(())
    }

    #[test]
    fn each_revision_dimension_changes_freshness_independently() -> TestResult {
        let original = revision()?;
        assert_eq!(original.ensure_current(&revision()?), Ok(()));
        let changes = [
            ("other", "input-v1", "a", "roster-v1", "b"),
            ("document-private", "input-v2", "a", "roster-v1", "b"),
            ("document-private", "input-v1", "c", "roster-v1", "b"),
            ("document-private", "input-v1", "a", "roster-v2", "b"),
            ("document-private", "input-v1", "a", "roster-v1", "d"),
        ];
        for (document, input_revision, input_hash, roster_revision, roster_hash) in changes {
            let current = WbsLocalInputRevision::new(
                document.into(),
                input_revision.into(),
                input_hash.repeat(64),
                roster_revision.into(),
                roster_hash.repeat(64),
            )?;
            assert_eq!(
                original.ensure_current(&current),
                Err(WbsCandidateError::Stale)
            );
        }
        Ok(())
    }

    #[test]
    fn model_provenance_preserves_registration_and_accepts_a_bounded_provider_model() -> TestResult
    {
        let model = WbsLocalModelProvenance::new(
            "c".repeat(64),
            "provider-private".into(),
            "vendor/model-private".into(),
            "prompt-v1".into(),
        )?;
        assert_eq!(model.provider_selection_digest(), "c".repeat(64));
        assert_eq!(model.provider_name(), "provider-private");
        assert_eq!(model.model(), "vendor/model-private");
        assert_eq!(model.prompt_revision(), "prompt-v1");
        let boundary = WbsLocalModelProvenance::new(
            "c".repeat(64),
            "provider".into(),
            "m".repeat(128),
            "prompt-v1".into(),
        )?;
        assert_eq!(boundary.model().len(), 128);
        Ok(())
    }

    #[test]
    fn model_provenance_rejects_missing_pins_and_never_trims_model_input() {
        let invalid = [
            ("C".repeat(64), "provider", "model".into(), "prompt-v1"),
            ("c".repeat(64), "", "model".into(), "prompt-v1"),
            ("c".repeat(64), "provider", String::new(), "prompt-v1"),
            ("c".repeat(64), "provider", "model".into(), ""),
            ("c".repeat(64), "provider", " model".into(), "prompt-v1"),
            ("c".repeat(64), "provider", "model ".into(), "prompt-v1"),
            (
                "c".repeat(64),
                "provider",
                "model\nname".into(),
                "prompt-v1",
            ),
            ("c".repeat(64), "provider", "m".repeat(129), "prompt-v1"),
        ];
        for (digest, provider, model, prompt) in invalid {
            assert_eq!(
                WbsLocalModelProvenance::new(digest, provider.into(), model, prompt.into()),
                Err(WbsCandidateError::InvalidInput)
            );
        }
    }

    #[test]
    fn local_debug_redacts_registration_revisions_and_model_metadata() -> TestResult {
        let revision = revision()?;
        let model = WbsLocalModelProvenance::new(
            "c".repeat(64),
            "provider-private".into(),
            "model-private".into(),
            "prompt-private".into(),
        )?;
        assert_eq!(format!("{revision:?}"), "WbsLocalInputRevision([REDACTED])");
        assert_eq!(format!("{model:?}"), "WbsLocalModelProvenance([REDACTED])");
        Ok(())
    }

    fn task() -> Result<WbsLocalTask, WbsCandidateError> {
        WbsLocalTask::new(
            "row-a".into(),
            "Explicit synthetic task\nOnly this input.".into(),
        )
    }

    fn member() -> Result<WbsLocalRosterMember, WbsCandidateError> {
        WbsLocalRosterMember::new(
            "person-a".into(),
            "Synthetic Assignee".into(),
            vec![WbsLocalEvidence::new(
                "evidence-a".into(),
                "Explicit synthetic fact".into(),
            )?],
        )
    }

    #[test]
    fn explicit_task_and_evidence_text_preserve_permitted_whitespace_and_byte_bounds() -> TestResult
    {
        assert_eq!(task()?.wbs_item_id(), "row-a");
        assert_eq!(
            task()?.task_text(),
            "Explicit synthetic task\nOnly this input."
        );
        let task = WbsLocalTask::new("row-b".into(), " task\r\n\tcontinuation ".into())?;
        assert_eq!(task.task_text(), " task\r\n\tcontinuation ");
        let boundary = WbsLocalTask::new("row-b".into(), "t".repeat(4096))?;
        assert_eq!(boundary.task_text().len(), 4096);
        let evidence = WbsLocalEvidence::new("fact-b".into(), "e".repeat(512))?;
        assert_eq!(evidence.evidence_id(), "fact-b");
        assert_eq!(evidence.description().len(), 512);
        assert!(explicit_text("한", 3));
        assert!(!explicit_text("한", 2));
        for text in ["", "   ", "\r\n\t", "bad\0text", "bad\u{1b}text"] {
            assert_eq!(
                WbsLocalTask::new("row".into(), text.into()),
                Err(WbsCandidateError::InvalidInput)
            );
            assert_eq!(
                WbsLocalEvidence::new("fact".into(), text.into()),
                Err(WbsCandidateError::InvalidInput)
            );
        }
        assert_eq!(
            WbsLocalTask::new(String::new(), "task".into()),
            Err(WbsCandidateError::InvalidInput)
        );
        assert_eq!(
            WbsLocalEvidence::new(String::new(), "fact".into()),
            Err(WbsCandidateError::InvalidInput)
        );
        assert_eq!(
            WbsLocalEvidence::new("fact".into(), "e".repeat(513)),
            Err(WbsCandidateError::InvalidInput)
        );
        Ok(())
    }

    #[test]
    fn roster_text_cannot_become_a_formula_or_unbounded_input() -> TestResult {
        assert_eq!(member()?.display_name(), "Synthetic Assignee");
        for name in [
            "=1", "+1", "-1", "@name", "'name", " Name", "Name ", "Name\n", "",
        ] {
            assert_eq!(
                WbsLocalRosterMember::new("person-a".into(), name.into(), vec![]),
                Err(WbsCandidateError::InvalidInput)
            );
        }
        let boundary = WbsLocalRosterMember::new("person-a".into(), "n".repeat(256), vec![])?;
        assert_eq!(boundary.display_name().len(), 256);
        assert!(boundary.evidence().is_empty());
        assert_eq!(
            WbsLocalRosterMember::new("person-a".into(), "n".repeat(257), vec![]),
            Err(WbsCandidateError::InvalidInput)
        );
        assert_eq!(
            WbsLocalRosterMember::new(String::new(), "Name".into(), vec![]),
            Err(WbsCandidateError::InvalidInput)
        );
        assert_eq!(
            WbsLocalTask::new("row-a".into(), "a".repeat(4097)),
            Err(WbsCandidateError::InvalidInput)
        );
        assert_eq!(
            WbsLocalTask::new("row-a".into(), "bad\0text".into()),
            Err(WbsCandidateError::InvalidInput)
        );
        Ok(())
    }

    #[test]
    fn roster_member_preserves_bounded_facts_and_rejects_duplicate_evidence() -> TestResult {
        let facts = (0..8)
            .map(|i| WbsLocalEvidence::new(format!("fact-{i}"), "Explicit fact".into()))
            .collect::<Result<Vec<_>, _>>()?;
        let member = WbsLocalRosterMember::new(
            "person-a".into(),
            "Synthetic Assignee".into(),
            facts.clone(),
        )?;
        assert_eq!(member.person_id(), "person-a");
        assert_eq!(member.evidence(), facts);
        let fact = WbsLocalEvidence::new("fact-a".into(), "One fact".into())?;
        assert_eq!(
            WbsLocalRosterMember::new(
                "person-a".into(),
                "Synthetic Assignee".into(),
                vec![fact.clone(), fact]
            ),
            Err(WbsCandidateError::InvalidInput)
        );
        let too_many = (0..9)
            .map(|i| WbsLocalEvidence::new(format!("fact-{i}"), "Explicit fact".into()))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            WbsLocalRosterMember::new("person-a".into(), "Synthetic Assignee".into(), too_many),
            Err(WbsCandidateError::InvalidInput)
        );
        Ok(())
    }

    #[test]
    fn local_debug_redacts_actual_task_person_and_evidence_text() -> TestResult {
        let task = task()?;
        let member = member()?;
        let evidence = &member.evidence()[0];
        assert_eq!(format!("{task:?}"), "WbsLocalTask([REDACTED])");
        assert_eq!(format!("{member:?}"), "WbsLocalRosterMember([REDACTED])");
        assert_eq!(format!("{evidence:?}"), "WbsLocalEvidence([REDACTED])");
        Ok(())
    }

    #[test]
    fn local_input_preserves_explicit_facts_and_an_empty_roster() -> TestResult {
        let input = WbsLocalDocumentInput::new(revision()?, vec![task()?], vec![member()?], true)?;
        assert_eq!(input.task("row-a"), Some(&task()?));
        assert_eq!(input.member("person-a"), Some(&member()?));
        assert_eq!(input.member("invented-person"), None);
        assert!(input.synthetic());
        let empty = WbsLocalDocumentInput::new(revision()?, vec![task()?], vec![], true)?;
        assert_eq!(empty.roster(), &[]);
        assert_eq!(empty.task("row-a"), Some(&task()?));
        Ok(())
    }

    #[test]
    fn duplicate_rows_people_and_evidence_are_rejected_separately() -> TestResult {
        assert_eq!(
            WbsLocalDocumentInput::new(revision()?, vec![task()?, task()?], vec![member()?], true),
            Err(WbsCandidateError::InvalidInput)
        );
        assert_eq!(
            WbsLocalDocumentInput::new(
                revision()?,
                vec![task()?],
                vec![member()?, member()?],
                true
            ),
            Err(WbsCandidateError::InvalidInput)
        );
        let fact = WbsLocalEvidence::new("fact-a".into(), "One fact".into())?;
        assert_eq!(
            WbsLocalRosterMember::new(
                "person-a".into(),
                "Synthetic Assignee".into(),
                vec![fact.clone(), fact]
            ),
            Err(WbsCandidateError::InvalidInput)
        );
        Ok(())
    }

    #[test]
    fn local_debug_redacts_document_people_task_and_model_metadata() -> TestResult {
        let input = WbsLocalDocumentInput::new(revision()?, vec![task()?], vec![member()?], true)?;
        let model = WbsLocalModelProvenance::new(
            "c".repeat(64),
            "provider-fixture".into(),
            "model-fixture".into(),
            "prompt-v1".into(),
        )?;
        let result = WbsLocalResultSnapshot::new(
            WbsCandidateSnapshotRef::new("result-a".into(), "1".into(), "d".repeat(64))?,
            revision()?,
            model.clone(),
        )?;
        for debug in [
            format!("{input:?}"),
            format!("{:?}", task()?),
            format!("{:?}", member()?),
            format!("{model:?}"),
            format!("{result:?}"),
            format!("{:?}", revision()?),
        ] {
            assert!(debug.ends_with("([REDACTED])"));
            for text in [
                "document-private",
                "Synthetic Assignee",
                "Explicit",
                "provider-fixture",
                "model-fixture",
            ] {
                assert!(!debug.contains(text));
            }
        }
        Ok(())
    }

    #[test]
    fn document_input_preserves_revisions_and_enforces_row_and_roster_counts() -> TestResult {
        let tasks = (0..64)
            .map(|i| WbsLocalTask::new(format!("row-{i}"), "task".into()))
            .collect::<Result<Vec<_>, _>>()?;
        let roster = (0..32)
            .map(|i| WbsLocalRosterMember::new(format!("person-{i}"), "Name".into(), vec![]))
            .collect::<Result<Vec<_>, _>>()?;
        let input = WbsLocalDocumentInput::new(revision()?, tasks.clone(), roster.clone(), false)?;
        assert_eq!(input.revision(), &revision()?);
        assert_eq!(input.tasks(), tasks);
        assert_eq!(input.roster(), roster);
        assert!(!input.synthetic());
        assert_eq!(input.task("missing"), None);
        assert_eq!(
            WbsLocalDocumentInput::new(revision()?, vec![], vec![], false),
            Err(WbsCandidateError::InvalidInput)
        );
        let mut too_many_tasks = tasks;
        too_many_tasks.push(WbsLocalTask::new("row-extra".into(), "task".into())?);
        assert_eq!(
            WbsLocalDocumentInput::new(revision()?, too_many_tasks, vec![], false),
            Err(WbsCandidateError::InvalidInput)
        );
        let mut too_many_people = roster;
        too_many_people.push(WbsLocalRosterMember::new(
            "person-extra".into(),
            "Name".into(),
            vec![],
        )?);
        assert_eq!(
            WbsLocalDocumentInput::new(revision()?, vec![task()?], too_many_people, false),
            Err(WbsCandidateError::InvalidInput)
        );
        Ok(())
    }

    #[test]
    fn aggregate_input_budget_counts_tasks_names_and_evidence_at_the_exact_boundary() -> TestResult
    {
        let tasks = (0..16)
            .map(|i| WbsLocalTask::new(format!("row-{i}"), "t".repeat(4096)))
            .collect::<Result<Vec<_>, _>>()?;
        let accepted = WbsLocalDocumentInput::new(revision()?, tasks.clone(), vec![], true)?;
        assert_eq!(accepted.tasks(), tasks);
        assert_eq!(accepted.roster(), &[]);
        assert_eq!(
            WbsLocalDocumentInput::new(
                revision()?,
                tasks,
                vec![WbsLocalRosterMember::new("p".into(), "N".into(), vec![])?],
                true
            ),
            Err(WbsCandidateError::InvalidInput)
        );
        let mut tasks = (0..15)
            .map(|i| WbsLocalTask::new(format!("row-{i}"), "t".repeat(4096)))
            .collect::<Result<Vec<_>, _>>()?;
        tasks.push(WbsLocalTask::new("row-15".into(), "t".repeat(4095))?);
        let roster_at_limit = vec![WbsLocalRosterMember::new("p".into(), "N".into(), vec![])?];
        let accepted =
            WbsLocalDocumentInput::new(revision()?, tasks.clone(), roster_at_limit.clone(), true)?;
        assert_eq!(accepted.tasks(), tasks);
        assert_eq!(accepted.roster(), roster_at_limit);
        let roster = vec![WbsLocalRosterMember::new(
            "p".into(),
            "N".into(),
            vec![WbsLocalEvidence::new("f".into(), "F".into())?],
        )?];
        assert_eq!(
            WbsLocalDocumentInput::new(revision()?, tasks, roster, true),
            Err(WbsCandidateError::InvalidInput)
        );
        Ok(())
    }

    #[test]
    fn local_result_snapshot_preserves_fixed_result_and_requires_lowercase_hashes() -> TestResult {
        let reference =
            WbsCandidateSnapshotRef::new("result-a".into(), "r1".into(), "d".repeat(64))?;
        let model = WbsLocalModelProvenance::new(
            "c".repeat(64),
            "provider-a".into(),
            "model-a".into(),
            "prompt-v1".into(),
        )?;
        let result = WbsLocalResultSnapshot::new(reference.clone(), revision()?, model.clone())?;
        assert_eq!(result.reference(), &reference);
        assert_eq!(result.input(), &revision()?);
        assert_eq!(result.model(), &model);
        let upper = WbsCandidateSnapshotRef::new("result-a".into(), "r1".into(), "D".repeat(64))?;
        assert_eq!(
            WbsLocalResultSnapshot::new(upper, revision()?, model),
            Err(WbsCandidateError::InvalidResponse)
        );
        Ok(())
    }
}
