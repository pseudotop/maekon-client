//! Local-first XLSX package editor for approved TMD projections (#10358).
//!
//! The adapter deliberately edits only the selected worksheet XML part. Every
//! other ZIP member is copied in its original compressed form, avoiding the
//! broad read/write surface and supply-chain cost of a general spreadsheet
//! object model. Header checks run before any output is assembled.
// OOS-TBD: ADR-013 file split after #10358 isolates package parsing, XML patching, and tests.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Cursor, Read, Write};
use std::path::Path;

use icu_normalizer::ComposingNormalizerBorrowed;
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer, XmlVersion};
use tempfile::NamedTempFile;
use thiserror::Error;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use serde::Serialize;
use sha2::{Digest, Sha256};

pub const MAX_TEMPLATE_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_DECOMPRESSED_BYTES: u64 = 100 * 1024 * 1024;
const MAX_ZIP_ENTRIES: usize = 2_048;
const MAX_XML_PART_BYTES: u64 = 32 * 1024 * 1024;
const MAX_EXCEL_ROW: u32 = 1_048_576;
const MAX_EXCEL_COLUMN: u32 = 16_384;
const WORKBOOK_PART: &str = "xl/workbook.xml";
const WORKBOOK_RELS_PART: &str = "xl/_rels/workbook.xml.rels";
const SHARED_STRINGS_PART: &str = "xl/sharedStrings.xml";

#[derive(Debug, Error)]
pub enum XlsxWriterError {
    #[error("template exceeds the compressed size limit: {actual} > {limit}")]
    TemplateTooLarge { actual: usize, limit: usize },
    #[error("template exceeds the decompressed size limit: {actual} > {limit}")]
    DecompressedTooLarge { actual: u64, limit: u64 },
    #[error("template has too many ZIP entries: {actual} > {limit}")]
    TooManyEntries { actual: usize, limit: usize },
    #[error("unsafe ZIP member path: {0}")]
    UnsafeZipPath(String),
    #[error("duplicate ZIP member: {0}")]
    DuplicateZipMember(String),
    #[error("unsupported active-content or external-link part: {0}")]
    UnsafeWorkbookPart(String),
    #[error("required XLSX package part is missing: {0}")]
    MissingPart(String),
    #[error("worksheet is missing: {0}")]
    MissingWorksheet(String),
    #[error("invalid cell reference: {0}")]
    InvalidCellReference(String),
    #[error("duplicate cell update: {0}")]
    DuplicateCellUpdate(String),
    #[error("worksheet rows or cells are not in ascending order")]
    NonCanonicalWorksheetOrder,
    #[error("invalid numeric cell value")]
    InvalidNumber,
    #[error("unsafe system formula: {0}")]
    UnsafeFormula(String),
    #[error("unsupported XML construct: {0}")]
    UnsupportedXml(String),
    #[error("malformed XLSX contract: {0}")]
    Malformed(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
    #[error(transparent)]
    Xml(#[from] quick_xml::Error),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeaderExpectation {
    pub column: String,
    pub expected: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CellValue {
    Text(String),
    Integer(i64),
    Number(f64),
    /// A trusted formula generated from validated row/column coordinates.
    Formula(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct CellUpdate {
    pub coordinate: String,
    pub value: CellValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeaderDrift {
    pub expected: Vec<String>,
    pub actual: Vec<String>,
    pub first_mismatch_column: String,
}

#[derive(Debug, Eq, PartialEq)]
pub enum XlsxFillOutcome {
    Written {
        bytes: Vec<u8>,
        escaped_cell_count: usize,
    },
    HeaderDrift(HeaderDrift),
}

#[derive(Debug, Serialize)]
struct WorkbookStructure<'a> {
    version: u8,
    defined_names_xml: String,
    sheets: Vec<WorksheetStructure<'a>>,
}

#[derive(Debug, Serialize)]
struct WorksheetStructure<'a> {
    name: &'a str,
    headers: Vec<Option<String>>,
    columns: Vec<BTreeMap<String, String>>,
    merged_cells: Vec<String>,
    freeze_panes: Option<String>,
}

#[derive(Clone, Debug)]
struct ValidatedUpdate {
    coordinate: String,
    column: u32,
    row: u32,
    value: CellValue,
}

type RowUpdates = BTreeMap<u32, ValidatedUpdate>;
type ValidatedUpdates = BTreeMap<u32, RowUpdates>;

/// Build a SUM formula exclusively from validated coordinates.
pub fn rollup_sum_formula(column: &str, child_rows: &[u32]) -> Result<String, XlsxWriterError> {
    let column = canonical_column(column)?;
    if child_rows.is_empty()
        || child_rows
            .iter()
            .any(|row| *row == 0 || *row > MAX_EXCEL_ROW)
    {
        return Err(XlsxWriterError::UnsafeFormula(format!(
            "SUM column={column} rows={child_rows:?}"
        )));
    }
    let cells = child_rows
        .iter()
        .map(|row| format!("{column}{row}"))
        .collect::<Vec<_>>()
        .join(",");
    Ok(format!("SUM({cells})"))
}

/// Validate headers and fill only the selected worksheet part.
pub fn fill_workbook(
    input: &[u8],
    sheet_name: &str,
    headers: &[HeaderExpectation],
    updates: &[CellUpdate],
) -> Result<XlsxFillOutcome, XlsxWriterError> {
    validate_archive(input)?;

    let mut archive = ZipArchive::new(Cursor::new(input))?;
    let mut budget = PartReadBudget::new();
    let workbook_xml = read_required_part(&mut archive, WORKBOOK_PART, &mut budget)?;
    let rels_xml = read_required_part(&mut archive, WORKBOOK_RELS_PART, &mut budget)?;
    let worksheet_part = resolve_worksheet_part(&workbook_xml, &rels_xml, sheet_name)?;
    let worksheet_xml = read_required_part(&mut archive, &worksheet_part, &mut budget)?;
    let shared_strings = match read_optional_part(&mut archive, SHARED_STRINGS_PART, &mut budget)? {
        Some(xml) => parse_shared_strings(&xml)?,
        None => Vec::new(),
    };

    let drift = detect_header_drift(&worksheet_xml, &shared_strings, headers)?;
    if let Some(drift) = drift {
        return Ok(XlsxFillOutcome::HeaderDrift(drift));
    }

    let (validated_updates, escaped_cell_count) = validate_updates(updates)?;
    let updated_xml = patch_worksheet(&worksheet_xml, validated_updates)?;
    let bytes = rebuild_archive(input, &worksheet_part, &updated_xml)?;
    Ok(XlsxFillOutcome::Written {
        bytes,
        escaped_cell_count,
    })
}

/// Hash the workbook shape observed before any value update.
///
/// The canonical payload excludes data-row values while retaining worksheet
/// order/names, the complete row-1 header extent, column dimension attributes,
/// merge ranges, freeze panes and workbook defined names. This makes the hash a
/// structural evidence anchor rather than a second artifact-byte digest.
pub fn template_structure_hash(input: &[u8]) -> Result<String, XlsxWriterError> {
    validate_archive(input)?;
    let mut archive = ZipArchive::new(Cursor::new(input))?;
    let mut budget = PartReadBudget::new();
    let workbook_xml = read_required_part(&mut archive, WORKBOOK_PART, &mut budget)?;
    let rels_xml = read_required_part(&mut archive, WORKBOOK_RELS_PART, &mut budget)?;
    let shared_strings = match read_optional_part(&mut archive, SHARED_STRINGS_PART, &mut budget)? {
        Some(xml) => parse_shared_strings(&xml)?,
        None => Vec::new(),
    };
    let sheet_names = workbook_sheet_names(&workbook_xml)?;
    let defined_names_xml = workbook_defined_names(&workbook_xml)?;
    let mut sheets = Vec::with_capacity(sheet_names.len());
    for name in &sheet_names {
        let part = resolve_worksheet_part(&workbook_xml, &rels_xml, name)?;
        let xml = read_required_part(&mut archive, &part, &mut budget)?;
        sheets.push(worksheet_structure(name, &xml, &shared_strings)?);
    }
    let canonical = serde_json::to_vec(&WorkbookStructure {
        version: 1,
        defined_names_xml,
        sheets,
    })
    .map_err(|error| {
        XlsxWriterError::Malformed(format!("serialize workbook structure: {error}"))
    })?;
    let digest = Sha256::digest(canonical);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn workbook_sheet_names(workbook_xml: &[u8]) -> Result<Vec<String>, XlsxWriterError> {
    let mut names = Vec::new();
    let mut reader = secure_reader(workbook_xml);
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Empty(start) | Event::Start(start)
                if local_name(start.name().as_ref()) == "sheet" =>
            {
                let name = attribute_value(&start, "name")?
                    .ok_or_else(|| XlsxWriterError::Malformed("sheet has no name".to_owned()))?;
                names.push(name);
                // Every worksheet needs its own package part, so a workbook cannot
                // list more sheets than the archive may hold entries. The cap also
                // bounds the caller, which rescans workbook.xml once per sheet.
                if names.len() > MAX_ZIP_ENTRIES {
                    return Err(XlsxWriterError::Malformed(format!(
                        "workbook lists more than {MAX_ZIP_ENTRIES} worksheets"
                    )));
                }
            }
            Event::DocType(_) => {
                return Err(XlsxWriterError::UnsupportedXml(
                    "DOCTYPE in workbook.xml".to_owned(),
                ));
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if names.is_empty() {
        return Err(XlsxWriterError::Malformed(
            "workbook has no worksheets".to_owned(),
        ));
    }
    Ok(names)
}

fn workbook_defined_names(workbook_xml: &[u8]) -> Result<String, XlsxWriterError> {
    let events = parse_xml_events(workbook_xml, WORKBOOK_PART)?;
    let Some(start) = events
        .iter()
        .position(|event| is_start(event, "definedNames"))
    else {
        return Ok(String::new());
    };
    let end = matching_end(&events, start, "definedNames")?;
    let mut writer = Writer::new(Vec::new());
    for event in &events[start..=end] {
        writer.write_event(event.clone())?;
    }
    String::from_utf8(writer.into_inner()).map_err(|error| {
        XlsxWriterError::Malformed(format!("defined names are not UTF-8: {error}"))
    })
}

fn worksheet_structure<'a>(
    name: &'a str,
    worksheet_xml: &[u8],
    shared_strings: &[String],
) -> Result<WorksheetStructure<'a>, XlsxWriterError> {
    let events = parse_xml_events(worksheet_xml, "worksheet")?;
    let mut header_cells = BTreeMap::<u32, String>::new();
    let mut max_column = 0_u32;
    let mut columns = Vec::new();
    let mut merged_cells = Vec::new();
    let mut freeze_panes = None;
    let mut index = 0;
    while index < events.len() {
        match &events[index] {
            Event::Start(start) if local_name(start.name().as_ref()) == "c" => {
                let coordinate = event_attribute(start, "r")?.ok_or_else(|| {
                    XlsxWriterError::Malformed("cell has no r attribute".to_owned())
                })?;
                let (column, row) = parse_cell_reference(&coordinate)?;
                max_column = max_column.max(column);
                let end = matching_end(&events, index, "c")?;
                if row == 1 {
                    header_cells.insert(
                        column,
                        cell_text(start, &events[index + 1..end], shared_strings)?,
                    );
                }
                index = end + 1;
                continue;
            }
            Event::Empty(start) if local_name(start.name().as_ref()) == "c" => {
                let coordinate = event_attribute(start, "r")?.ok_or_else(|| {
                    XlsxWriterError::Malformed("cell has no r attribute".to_owned())
                })?;
                let (column, row) = parse_cell_reference(&coordinate)?;
                max_column = max_column.max(column);
                if row == 1 {
                    header_cells.insert(column, String::new());
                }
            }
            Event::Empty(start) | Event::Start(start)
                if local_name(start.name().as_ref()) == "col" =>
            {
                let mut attributes = BTreeMap::new();
                for attribute in start.attributes().with_checks(false) {
                    let attribute = attribute.map_err(|error| {
                        XlsxWriterError::Malformed(format!("invalid column attribute: {error}"))
                    })?;
                    let key = local_name(attribute.key.as_ref()).to_owned();
                    let value = attribute
                        .normalized_value(XmlVersion::Explicit1_0)
                        .map_err(|error| XlsxWriterError::Malformed(error.to_string()))?
                        .into_owned();
                    attributes.insert(key, value);
                }
                columns.push(attributes);
            }
            Event::Empty(start) | Event::Start(start)
                if local_name(start.name().as_ref()) == "mergeCell" =>
            {
                if let Some(reference) = event_attribute(start, "ref")? {
                    for coordinate in reference.split(':') {
                        let (column, _) = parse_cell_reference(coordinate)?;
                        max_column = max_column.max(column);
                    }
                    merged_cells.push(reference);
                }
            }
            Event::Empty(start) | Event::Start(start)
                if local_name(start.name().as_ref()) == "pane" =>
            {
                freeze_panes = event_attribute(start, "topLeftCell")?;
            }
            _ => {}
        }
        index += 1;
    }
    merged_cells.sort();
    columns.sort_by(|left, right| left.get("min").cmp(&right.get("min")));
    let headers = (1..=max_column)
        .map(|column| header_cells.get(&column).cloned())
        .collect();
    Ok(WorksheetStructure {
        name,
        headers,
        columns,
        merged_cells,
        freeze_panes,
    })
}

/// Read a user-selected template without loading more than the size gate allows.
///
/// The metadata length rejects an oversized file before any read. The read itself
/// stops one byte past the limit, so a device, FIFO or still-growing file is
/// reported as too large instead of being buffered whole.
pub fn read_template_file(path: &Path) -> Result<Vec<u8>, XlsxWriterError> {
    let limit = MAX_TEMPLATE_BYTES as u64;
    let file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    if length > limit {
        return Err(XlsxWriterError::TemplateTooLarge {
            actual: usize::try_from(length).unwrap_or(usize::MAX),
            limit: MAX_TEMPLATE_BYTES,
        });
    }
    let mut bytes = Vec::with_capacity(length as usize);
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_TEMPLATE_BYTES {
        return Err(XlsxWriterError::TemplateTooLarge {
            actual: bytes.len(),
            limit: MAX_TEMPLATE_BYTES,
        });
    }
    Ok(bytes)
}

/// Persist completed bytes with a sibling temporary file and atomic replace.
pub fn persist_atomic(path: &Path, data: &[u8]) -> Result<(), XlsxWriterError> {
    let parent = path
        .parent()
        .ok_or_else(|| XlsxWriterError::Malformed("output path has no parent".to_owned()))?;
    if !parent.is_dir() {
        return Err(XlsxWriterError::Malformed(format!(
            "output parent does not exist: {}",
            parent.display()
        )));
    }

    let mut temporary = NamedTempFile::new_in(parent)?;
    temporary.write_all(data)?;
    temporary.as_file_mut().flush()?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| XlsxWriterError::Io(error.error))?;
    Ok(())
}

fn validate_archive(input: &[u8]) -> Result<(), XlsxWriterError> {
    if input.len() > MAX_TEMPLATE_BYTES {
        return Err(XlsxWriterError::TemplateTooLarge {
            actual: input.len(),
            limit: MAX_TEMPLATE_BYTES,
        });
    }
    let mut archive = ZipArchive::new(Cursor::new(input))?;
    if archive.len() > MAX_ZIP_ENTRIES {
        return Err(XlsxWriterError::TooManyEntries {
            actual: archive.len(),
            limit: MAX_ZIP_ENTRIES,
        });
    }

    let mut names = HashSet::with_capacity(archive.len());
    let mut decompressed = 0_u64;
    for index in 0..archive.len() {
        let file = archive.by_index(index)?;
        let name = file.name()?.into_owned();
        validate_member_name(&name)?;
        if !names.insert(name.clone()) {
            return Err(XlsxWriterError::DuplicateZipMember(name));
        }
        if is_unsafe_workbook_part(&name) {
            return Err(XlsxWriterError::UnsafeWorkbookPart(name));
        }
        decompressed = decompressed.saturating_add(file.size());
        if decompressed > MAX_DECOMPRESSED_BYTES {
            return Err(XlsxWriterError::DecompressedTooLarge {
                actual: decompressed,
                limit: MAX_DECOMPRESSED_BYTES,
            });
        }
    }
    Ok(())
}

fn validate_member_name(name: &str) -> Result<(), XlsxWriterError> {
    let path = Path::new(name);
    let unsafe_name = name.starts_with('/')
        || name.starts_with('\\')
        || name.contains('\\')
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        });
    if unsafe_name {
        return Err(XlsxWriterError::UnsafeZipPath(name.to_owned()));
    }
    Ok(())
}

fn is_unsafe_workbook_part(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "xl/vbaproject.bin"
        || lower.starts_with("xl/externallinks/")
        || lower.contains("/activex/")
        || lower.contains("/embeddings/")
}

/// Decompressed bytes one workbook operation may read across all of its parts.
///
/// The declared sizes that `validate_archive` and `read_optional_part` check come
/// from the template itself. zip 8 does not stop a decompressor at the declared
/// size, and its CRC check runs only after `read_to_end` has buffered the whole
/// stream, so those checks bound nothing on their own. Sheets may also alias one
/// part, which a per-entry total never sees. Every part read therefore draws the
/// bytes it actually produced from this budget.
struct PartReadBudget {
    remaining: u64,
}

impl PartReadBudget {
    fn new() -> Self {
        Self {
            remaining: MAX_DECOMPRESSED_BYTES,
        }
    }
}

fn read_required_part<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
    budget: &mut PartReadBudget,
) -> Result<Vec<u8>, XlsxWriterError> {
    read_optional_part(archive, name, budget)?
        .ok_or_else(|| XlsxWriterError::MissingPart(name.to_owned()))
}

fn read_optional_part<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
    budget: &mut PartReadBudget,
) -> Result<Option<Vec<u8>>, XlsxWriterError> {
    let Ok(file) = archive.by_name(name) else {
        return Ok(None);
    };
    if file.size() > MAX_XML_PART_BYTES {
        return Err(XlsxWriterError::DecompressedTooLarge {
            actual: file.size(),
            limit: MAX_XML_PART_BYTES,
        });
    }
    let cap = MAX_XML_PART_BYTES.min(budget.remaining);
    let mut body = Vec::with_capacity(file.size().min(cap) as usize);
    // Read one byte past the cap so an overrun is reported instead of truncated.
    file.take(cap + 1).read_to_end(&mut body)?;
    let read = body.len() as u64;
    if read > MAX_XML_PART_BYTES {
        return Err(XlsxWriterError::DecompressedTooLarge {
            actual: read,
            limit: MAX_XML_PART_BYTES,
        });
    }
    if read > budget.remaining {
        return Err(XlsxWriterError::DecompressedTooLarge {
            actual: MAX_DECOMPRESSED_BYTES - budget.remaining + read,
            limit: MAX_DECOMPRESSED_BYTES,
        });
    }
    budget.remaining -= read;
    Ok(Some(body))
}

fn resolve_worksheet_part(
    workbook_xml: &[u8],
    rels_xml: &[u8],
    sheet_name: &str,
) -> Result<String, XlsxWriterError> {
    let wanted = nfc(sheet_name);
    let mut relationship_id = None;
    let mut reader = secure_reader(workbook_xml);
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Empty(start) | Event::Start(start)
                if local_name(start.name().as_ref()) == "sheet" =>
            {
                let name = attribute_value(&start, "name")?;
                if name.as_deref().map(nfc).as_deref() == Some(wanted.as_str()) {
                    relationship_id = attribute_value(&start, "id")?;
                    break;
                }
            }
            Event::DocType(_) => {
                return Err(XlsxWriterError::UnsupportedXml(
                    "DOCTYPE in workbook.xml".to_owned(),
                ));
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let relationship_id =
        relationship_id.ok_or_else(|| XlsxWriterError::MissingWorksheet(sheet_name.to_owned()))?;

    let mut reader = secure_reader(rels_xml);
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Empty(start) | Event::Start(start)
                if local_name(start.name().as_ref()) == "Relationship" =>
            {
                let id = attribute_value(&start, "Id")?;
                if id.as_deref() == Some(relationship_id.as_str()) {
                    if attribute_value(&start, "TargetMode")?
                        .is_some_and(|mode| mode.eq_ignore_ascii_case("external"))
                    {
                        return Err(XlsxWriterError::UnsafeWorkbookPart(format!(
                            "external worksheet relationship {relationship_id}"
                        )));
                    }
                    let target = attribute_value(&start, "Target")?.ok_or_else(|| {
                        XlsxWriterError::Malformed(format!(
                            "worksheet relationship {relationship_id} has no target"
                        ))
                    })?;
                    return normalize_worksheet_target(&target);
                }
            }
            Event::DocType(_) => {
                return Err(XlsxWriterError::UnsupportedXml(
                    "DOCTYPE in workbook relationships".to_owned(),
                ));
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Err(XlsxWriterError::Malformed(format!(
        "worksheet relationship is missing: {relationship_id}"
    )))
}

fn normalize_worksheet_target(target: &str) -> Result<String, XlsxWriterError> {
    let target = target.trim_start_matches('/');
    let joined = if target.starts_with("xl/") {
        target.to_owned()
    } else {
        format!("xl/{target}")
    };
    validate_member_name(&joined)?;
    if !joined.starts_with("xl/worksheets/") || !joined.ends_with(".xml") {
        return Err(XlsxWriterError::Malformed(format!(
            "worksheet target is outside xl/worksheets: {target}"
        )));
    }
    Ok(joined)
}

fn parse_shared_strings(xml: &[u8]) -> Result<Vec<String>, XlsxWriterError> {
    let events = parse_xml_events(xml, SHARED_STRINGS_PART)?;
    let mut values = Vec::new();
    let mut index = 0;
    while index < events.len() {
        if is_start(&events[index], "si") {
            let end = matching_end(&events, index, "si")?;
            values.push(text_nodes(&events[index + 1..end], "t")?);
            index = end + 1;
        } else {
            index += 1;
        }
    }
    Ok(values)
}

fn detect_header_drift(
    worksheet_xml: &[u8],
    shared_strings: &[String],
    headers: &[HeaderExpectation],
) -> Result<Option<HeaderDrift>, XlsxWriterError> {
    if headers.is_empty() {
        return Ok(None);
    }
    let events = parse_xml_events(worksheet_xml, "worksheet")?;
    let mut cells = HashMap::new();
    let mut index = 0;
    while index < events.len() {
        match &events[index] {
            Event::Start(start) if local_name(start.name().as_ref()) == "c" => {
                let coordinate = event_attribute(start, "r")?.ok_or_else(|| {
                    XlsxWriterError::Malformed("cell has no r attribute".to_owned())
                })?;
                let (_, row) = parse_cell_reference(&coordinate)?;
                let end = matching_end(&events, index, "c")?;
                if row == 1 {
                    cells.insert(
                        coordinate,
                        cell_text(start, &events[index + 1..end], shared_strings)?,
                    );
                }
                index = end + 1;
            }
            Event::Empty(start) if local_name(start.name().as_ref()) == "c" => {
                let coordinate = event_attribute(start, "r")?.ok_or_else(|| {
                    XlsxWriterError::Malformed("cell has no r attribute".to_owned())
                })?;
                let (_, row) = parse_cell_reference(&coordinate)?;
                if row == 1 {
                    cells.insert(coordinate, String::new());
                }
                index += 1;
            }
            _ => index += 1,
        }
    }

    let expected = headers
        .iter()
        .map(|header| nfc(&header.expected))
        .collect::<Vec<_>>();
    let mut actual = Vec::with_capacity(headers.len());
    let mut first_mismatch_column = None;
    for (index, header) in headers.iter().enumerate() {
        let column = canonical_column(&header.column)?;
        let coordinate = format!("{column}1");
        let value = nfc(cells.get(&coordinate).map_or("", String::as_str));
        if value != expected[index] && first_mismatch_column.is_none() {
            first_mismatch_column = Some(column);
        }
        actual.push(value);
    }
    Ok(first_mismatch_column.map(|column| HeaderDrift {
        expected,
        actual,
        first_mismatch_column: column,
    }))
}

fn cell_text(
    start: &BytesStart<'_>,
    body: &[Event<'static>],
    shared_strings: &[String],
) -> Result<String, XlsxWriterError> {
    let cell_type = event_attribute(start, "t")?.unwrap_or_default();
    if cell_type == "inlineStr" {
        return text_nodes(body, "t");
    }
    let raw = text_nodes(body, "v")?;
    if cell_type == "s" {
        let index = raw.parse::<usize>().map_err(|_| {
            XlsxWriterError::Malformed(format!("shared string index is invalid: {raw}"))
        })?;
        return shared_strings.get(index).cloned().ok_or_else(|| {
            XlsxWriterError::Malformed(format!("shared string index is out of range: {index}"))
        });
    }
    Ok(raw)
}

fn text_nodes(events: &[Event<'static>], target: &str) -> Result<String, XlsxWriterError> {
    let mut depth = 0_u32;
    let mut value = String::new();
    for event in events {
        match event {
            Event::Start(start) if local_name(start.name().as_ref()) == target => depth += 1,
            Event::End(end) if local_name(end.name().as_ref()) == target => {
                depth = depth.saturating_sub(1);
            }
            Event::Text(text) if depth > 0 => {
                value.push_str(text.as_ref());
            }
            Event::CData(text) if depth > 0 => {
                value.push_str(text.as_ref());
            }
            Event::GeneralRef(reference) if depth > 0 => append_reference(&mut value, reference)?,
            _ => {}
        }
    }
    Ok(value)
}

fn append_reference(
    value: &mut String,
    reference: &quick_xml::events::BytesRef<'_>,
) -> Result<(), XlsxWriterError> {
    if let Some(character) = reference
        .resolve_char_ref()
        .map_err(|error| XlsxWriterError::Malformed(error.to_string()))?
    {
        value.push(character);
        return Ok(());
    }
    let entity = reference.as_ref();
    let resolved = quick_xml::escape::resolve_predefined_entity(entity)
        .ok_or_else(|| XlsxWriterError::UnsupportedXml(format!("unknown XML entity: {entity}")))?;
    value.push_str(resolved);
    Ok(())
}

fn validate_updates(updates: &[CellUpdate]) -> Result<(ValidatedUpdates, usize), XlsxWriterError> {
    let mut validated = ValidatedUpdates::new();
    let mut escaped_cell_count = 0;
    for update in updates {
        let (column, row) = parse_cell_reference(&update.coordinate)?;
        let column_name = column_name(column);
        let coordinate = format!("{column_name}{row}");
        let mut value = update.value.clone();
        match &mut value {
            CellValue::Text(text) => {
                if matches!(
                    text.chars().next(),
                    Some('=' | '+' | '-' | '@' | '\t' | '\r')
                ) {
                    text.insert(0, '\'');
                    escaped_cell_count += 1;
                }
            }
            CellValue::Number(number) if !number.is_finite() => {
                return Err(XlsxWriterError::InvalidNumber);
            }
            CellValue::Formula(formula) if !is_safe_formula(formula) => {
                return Err(XlsxWriterError::UnsafeFormula(formula.clone()));
            }
            _ => {}
        }
        let row_updates = validated.entry(row).or_default();
        if row_updates
            .insert(
                column,
                ValidatedUpdate {
                    coordinate: coordinate.clone(),
                    column,
                    row,
                    value,
                },
            )
            .is_some()
        {
            return Err(XlsxWriterError::DuplicateCellUpdate(coordinate));
        }
    }
    Ok((validated, escaped_cell_count))
}

fn is_safe_formula(formula: &str) -> bool {
    let Some(arguments) = formula
        .strip_prefix("SUM(")
        .and_then(|value| value.strip_suffix(')'))
    else {
        return false;
    };
    !arguments.is_empty()
        && arguments
            .split(',')
            .all(|coordinate| parse_cell_reference(coordinate).is_ok())
}

fn patch_worksheet(xml: &[u8], mut updates: ValidatedUpdates) -> Result<Vec<u8>, XlsxWriterError> {
    let events = parse_xml_events(xml, "worksheet")?;
    let sheet_data_start = events
        .iter()
        .position(|event| is_start(event, "sheetData"))
        .ok_or_else(|| XlsxWriterError::Malformed("worksheet has no sheetData".to_owned()))?;
    let sheet_data_end = matching_end(&events, sheet_data_start, "sheetData")?;

    let mut output = Vec::with_capacity(events.len() + updates.len() * 4);
    output.extend(events[..=sheet_data_start].iter().cloned());
    let mut index = sheet_data_start + 1;
    let mut previous_row = 0_u32;
    while index < sheet_data_end {
        match &events[index] {
            Event::Start(start) if local_name(start.name().as_ref()) == "row" => {
                let row = required_u32_attribute(start, "r", "row")?;
                if row <= previous_row {
                    return Err(XlsxWriterError::NonCanonicalWorksheetOrder);
                }
                append_missing_rows_before(&mut output, &mut updates, row)?;
                let end = matching_end(&events, index, "row")?;
                if let Some(row_updates) = updates.remove(&row) {
                    output.extend(patch_row(&events[index..=end], row, row_updates)?);
                } else {
                    output.extend(events[index..=end].iter().cloned());
                }
                previous_row = row;
                index = end + 1;
            }
            Event::Empty(start) if local_name(start.name().as_ref()) == "row" => {
                let row = required_u32_attribute(start, "r", "row")?;
                if row <= previous_row {
                    return Err(XlsxWriterError::NonCanonicalWorksheetOrder);
                }
                append_missing_rows_before(&mut output, &mut updates, row)?;
                if let Some(row_updates) = updates.remove(&row) {
                    output.extend(new_row_events(row, row_updates)?);
                } else {
                    output.push(events[index].clone());
                }
                previous_row = row;
                index += 1;
            }
            _ => {
                output.push(events[index].clone());
                index += 1;
            }
        }
    }
    for (row, row_updates) in updates {
        output.extend(new_row_events(row, row_updates)?);
    }
    output.extend(events[sheet_data_end..].iter().cloned());
    write_xml_events(&output)
}

fn append_missing_rows_before(
    output: &mut Vec<Event<'static>>,
    updates: &mut ValidatedUpdates,
    before: u32,
) -> Result<(), XlsxWriterError> {
    let rows = updates
        .range(..before)
        .map(|(row, _)| *row)
        .collect::<Vec<_>>();
    for row in rows {
        let row_updates = updates
            .remove(&row)
            .ok_or_else(|| XlsxWriterError::Malformed("pending row disappeared".to_owned()))?;
        output.extend(new_row_events(row, row_updates)?);
    }
    Ok(())
}

fn patch_row(
    events: &[Event<'static>],
    row: u32,
    mut updates: RowUpdates,
) -> Result<Vec<Event<'static>>, XlsxWriterError> {
    let Event::Start(start) = &events[0] else {
        return Err(XlsxWriterError::Malformed(
            "row does not start with Start".to_owned(),
        ));
    };
    let mut output = vec![clone_start_without(start, "spans")?];
    let mut index = 1;
    let mut previous_column = 0_u32;
    while index + 1 < events.len() {
        match &events[index] {
            Event::Start(cell) if local_name(cell.name().as_ref()) == "c" => {
                let coordinate = event_attribute(cell, "r")?.ok_or_else(|| {
                    XlsxWriterError::Malformed("cell has no r attribute".to_owned())
                })?;
                let (column, cell_row) = parse_cell_reference(&coordinate)?;
                if cell_row != row || column <= previous_column {
                    return Err(XlsxWriterError::NonCanonicalWorksheetOrder);
                }
                append_missing_cells_before(&mut output, &mut updates, column, None)?;
                let end = matching_end(events, index, "c")?;
                if let Some(update) = updates.remove(&column) {
                    output.extend(replacement_cell_events(Some(cell), &update)?);
                } else {
                    output.extend(events[index..=end].iter().cloned());
                }
                previous_column = column;
                index = end + 1;
            }
            Event::Empty(cell) if local_name(cell.name().as_ref()) == "c" => {
                let coordinate = event_attribute(cell, "r")?.ok_or_else(|| {
                    XlsxWriterError::Malformed("cell has no r attribute".to_owned())
                })?;
                let (column, cell_row) = parse_cell_reference(&coordinate)?;
                if cell_row != row || column <= previous_column {
                    return Err(XlsxWriterError::NonCanonicalWorksheetOrder);
                }
                append_missing_cells_before(&mut output, &mut updates, column, None)?;
                if let Some(update) = updates.remove(&column) {
                    output.extend(replacement_cell_events(Some(cell), &update)?);
                } else {
                    output.push(events[index].clone());
                }
                previous_column = column;
                index += 1;
            }
            _ => {
                output.push(events[index].clone());
                index += 1;
            }
        }
    }
    append_missing_cells_before(&mut output, &mut updates, u32::MAX, None)?;
    output.push(Event::End(BytesEnd::new("row")));
    Ok(output)
}

fn append_missing_cells_before(
    output: &mut Vec<Event<'static>>,
    updates: &mut RowUpdates,
    before: u32,
    existing: Option<&BytesStart<'_>>,
) -> Result<(), XlsxWriterError> {
    let columns = updates
        .range(..before)
        .map(|(column, _)| *column)
        .collect::<Vec<_>>();
    for column in columns {
        let update = updates
            .remove(&column)
            .ok_or_else(|| XlsxWriterError::Malformed("pending cell disappeared".to_owned()))?;
        output.extend(replacement_cell_events(existing, &update)?);
    }
    Ok(())
}

fn new_row_events(row: u32, updates: RowUpdates) -> Result<Vec<Event<'static>>, XlsxWriterError> {
    let mut start = BytesStart::new("row");
    let row_text = row.to_string();
    start.push_attribute(("r", row_text.as_str()));
    let mut events = vec![Event::Start(start.into_owned())];
    for update in updates.values() {
        events.extend(replacement_cell_events(None, update)?);
    }
    events.push(Event::End(BytesEnd::new("row")));
    Ok(events)
}

fn replacement_cell_events(
    existing: Option<&BytesStart<'_>>,
    update: &ValidatedUpdate,
) -> Result<Vec<Event<'static>>, XlsxWriterError> {
    debug_assert_eq!(
        parse_cell_reference(&update.coordinate)?,
        (update.column, update.row)
    );
    let mut cell = BytesStart::new("c");
    cell.push_attribute(("r", update.coordinate.as_str()));
    if let Some(existing) = existing {
        for attribute in existing.attributes() {
            let attribute =
                attribute.map_err(|error| XlsxWriterError::Malformed(error.to_string()))?;
            let key = attribute.key.as_ref();
            if local_name(key) == "r" || local_name(key) == "t" {
                continue;
            }
            let value = attribute
                .normalized_value(XmlVersion::Explicit1_0)
                .map_err(|error| XlsxWriterError::Malformed(error.to_string()))?;
            cell.push_attribute((key, value.as_ref()));
        }
    }

    let mut events = Vec::with_capacity(7);
    match &update.value {
        CellValue::Text(value) => {
            cell.push_attribute(("t", "inlineStr"));
            events.push(Event::Start(cell.into_owned()));
            events.push(Event::Start(BytesStart::new("is")));
            let mut text = BytesStart::new("t");
            text.push_attribute(("xml:space", "preserve"));
            events.push(Event::Start(text));
            events.push(Event::Text(BytesText::new(value).into_owned()));
            events.push(Event::End(BytesEnd::new("t")));
            events.push(Event::End(BytesEnd::new("is")));
            events.push(Event::End(BytesEnd::new("c")));
        }
        CellValue::Integer(value) => {
            events.push(Event::Start(cell.into_owned()));
            events.push(Event::Start(BytesStart::new("v")));
            events.push(Event::Text(BytesText::new(&value.to_string()).into_owned()));
            events.push(Event::End(BytesEnd::new("v")));
            events.push(Event::End(BytesEnd::new("c")));
        }
        CellValue::Number(value) => {
            if !value.is_finite() {
                return Err(XlsxWriterError::InvalidNumber);
            }
            events.push(Event::Start(cell.into_owned()));
            events.push(Event::Start(BytesStart::new("v")));
            events.push(Event::Text(BytesText::new(&value.to_string()).into_owned()));
            events.push(Event::End(BytesEnd::new("v")));
            events.push(Event::End(BytesEnd::new("c")));
        }
        CellValue::Formula(formula) => {
            if !is_safe_formula(formula) {
                return Err(XlsxWriterError::UnsafeFormula(formula.clone()));
            }
            events.push(Event::Start(cell.into_owned()));
            events.push(Event::Start(BytesStart::new("f")));
            events.push(Event::Text(BytesText::new(formula).into_owned()));
            events.push(Event::End(BytesEnd::new("f")));
            events.push(Event::End(BytesEnd::new("c")));
        }
    }
    Ok(events)
}

fn rebuild_archive(
    input: &[u8],
    worksheet_part: &str,
    updated_xml: &[u8],
) -> Result<Vec<u8>, XlsxWriterError> {
    let mut source = ZipArchive::new(Cursor::new(input))?;
    let cursor = Cursor::new(Vec::with_capacity(input.len()));
    let mut destination = ZipWriter::new(cursor);
    for index in 0..source.len() {
        let file = source.by_index(index)?;
        let name = file.name()?.into_owned();
        if name == worksheet_part {
            let options: SimpleFileOptions = file.options();
            destination.start_file(name, options)?;
            destination.write_all(updated_xml)?;
        } else {
            destination.raw_copy_file(file)?;
        }
    }
    Ok(destination.finish()?.into_inner())
}

fn parse_xml_events(xml: &[u8], part_name: &str) -> Result<Vec<Event<'static>>, XlsxWriterError> {
    let mut reader = secure_reader(xml);
    let mut buffer = Vec::new();
    let mut events = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::DocType(_) => {
                return Err(XlsxWriterError::UnsupportedXml(format!(
                    "DOCTYPE in {part_name}"
                )));
            }
            Event::Eof => break,
            event => events.push(event.into_owned()),
        }
        buffer.clear();
    }
    Ok(events)
}

fn write_xml_events(events: &[Event<'static>]) -> Result<Vec<u8>, XlsxWriterError> {
    let mut writer = Writer::new(Cursor::new(Vec::new()));
    for event in events {
        writer.write_event(event.borrow())?;
    }
    Ok(writer.into_inner().into_inner())
}

fn secure_reader(xml: &[u8]) -> Reader<Cursor<&[u8]>> {
    let mut reader = Reader::from_reader(Cursor::new(xml));
    reader.config_mut().trim_text(false);
    reader
}

fn matching_end(
    events: &[Event<'static>],
    start_index: usize,
    name: &str,
) -> Result<usize, XlsxWriterError> {
    let mut depth = 0_u32;
    for (index, event) in events.iter().enumerate().skip(start_index) {
        match event {
            Event::Start(start) if local_name(start.name().as_ref()) == name => depth += 1,
            Event::End(end) if local_name(end.name().as_ref()) == name => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Ok(index);
                }
            }
            _ => {}
        }
    }
    Err(XlsxWriterError::Malformed(format!(
        "missing end tag: {name}"
    )))
}

fn is_start(event: &Event<'_>, name: &str) -> bool {
    matches!(event, Event::Start(start) if local_name(start.name().as_ref()) == name)
}

fn local_name(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

fn attribute_value(start: &BytesStart<'_>, key: &str) -> Result<Option<String>, XlsxWriterError> {
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|error| XlsxWriterError::Malformed(error.to_string()))?;
        if local_name(attribute.key.as_ref()) == key {
            return Ok(Some(
                attribute
                    .normalized_value(XmlVersion::Explicit1_0)
                    .map_err(|error| XlsxWriterError::Malformed(error.to_string()))?
                    .into_owned(),
            ));
        }
    }
    Ok(None)
}

fn event_attribute(start: &BytesStart<'_>, key: &str) -> Result<Option<String>, XlsxWriterError> {
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|error| XlsxWriterError::Malformed(error.to_string()))?;
        if local_name(attribute.key.as_ref()) == key {
            return Ok(Some(
                attribute
                    .normalized_value(XmlVersion::Explicit1_0)
                    .map_err(|error| XlsxWriterError::Malformed(error.to_string()))?
                    .into_owned(),
            ));
        }
    }
    Ok(None)
}

fn required_u32_attribute(
    start: &BytesStart<'_>,
    key: &str,
    element: &str,
) -> Result<u32, XlsxWriterError> {
    let value = event_attribute(start, key)?
        .ok_or_else(|| XlsxWriterError::Malformed(format!("{element} has no numeric attribute")))?;
    value
        .parse::<u32>()
        .map_err(|_| XlsxWriterError::Malformed(format!("{element} attribute is invalid: {value}")))
}

fn clone_start_without(
    start: &BytesStart<'_>,
    omitted: &str,
) -> Result<Event<'static>, XlsxWriterError> {
    let qualified_name = start.name();
    let name = qualified_name.as_ref();
    let mut clone = BytesStart::new(name);
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|error| XlsxWriterError::Malformed(error.to_string()))?;
        if local_name(attribute.key.as_ref()) == omitted {
            continue;
        }
        let key = attribute.key.as_ref();
        let value = attribute
            .normalized_value(XmlVersion::Explicit1_0)
            .map_err(|error| XlsxWriterError::Malformed(error.to_string()))?;
        clone.push_attribute((key, value.as_ref()));
    }
    Ok(Event::Start(clone.into_owned()))
}

fn parse_cell_reference(reference: &str) -> Result<(u32, u32), XlsxWriterError> {
    let split = reference
        .find(|character: char| character.is_ascii_digit())
        .ok_or_else(|| XlsxWriterError::InvalidCellReference(reference.to_owned()))?;
    let (column, row) = reference.split_at(split);
    let column = column_index(column)?;
    let row = row
        .parse::<u32>()
        .map_err(|_| XlsxWriterError::InvalidCellReference(reference.to_owned()))?;
    if row == 0 || row > MAX_EXCEL_ROW {
        return Err(XlsxWriterError::InvalidCellReference(reference.to_owned()));
    }
    Ok((column, row))
}

fn canonical_column(column: &str) -> Result<String, XlsxWriterError> {
    Ok(column_name(column_index(column)?))
}

fn column_index(column: &str) -> Result<u32, XlsxWriterError> {
    if column.is_empty() || !column.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return Err(XlsxWriterError::InvalidCellReference(column.to_owned()));
    }
    let mut index = 0_u32;
    for byte in column.bytes() {
        index = index
            .checked_mul(26)
            .and_then(|value| value.checked_add(u32::from(byte.to_ascii_uppercase() - b'A') + 1))
            .ok_or_else(|| XlsxWriterError::InvalidCellReference(column.to_owned()))?;
    }
    if index == 0 || index > MAX_EXCEL_COLUMN {
        return Err(XlsxWriterError::InvalidCellReference(column.to_owned()));
    }
    Ok(index)
}

fn column_name(mut index: u32) -> String {
    let mut bytes = Vec::new();
    while index > 0 {
        index -= 1;
        bytes.push(b'A' + (index % 26) as u8);
        index /= 26;
    }
    bytes.reverse();
    bytes.into_iter().map(char::from).collect()
}

fn nfc(value: &str) -> String {
    ComposingNormalizerBorrowed::new_nfc()
        .normalize(value)
        .into_owned()
}

#[cfg(test)]
#[path = "tmd_xlsx_writer_tests.rs"]
mod tests;
