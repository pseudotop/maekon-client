use std::fs;

use tempfile::tempdir;
use zip::CompressionMethod;

use super::*;

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
  <Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
  <Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
  <Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>
</Types>"#;

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#;

const WORKBOOK_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/>
  <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
</Relationships>"#;

const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts>
  <fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="solid"><fgColor rgb="FF00FF00"/></patternFill></fill></fills>
  <borders count="2"><border/><border><left style="medium"><color rgb="FFFF0000"/></left><right style="medium"><color rgb="FFFF0000"/></right></border></borders>
  <cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>
  <cellXfs count="3"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/><xf numFmtId="0" fontId="0" fillId="1" borderId="1" xfId="0"/><xf numFmtId="2" fontId="0" fillId="0" borderId="0" xfId="0"/></cellXfs>
  <cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles>
</styleSheet>"#;

const SHEET_ONE: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <sheetData>
    <row r="1"><c r="A1" t="inlineStr"><is><t>WBS &#53076;&#46300;</t></is></c><c r="B1" t="inlineStr"><is><t>&#51089;&#50629;&#47749;</t></is></c></row>
    <row r="2" spans="1:4"><c r="A2" s="1" t="inlineStr"><is><t>old</t></is></c><c r="B2" s="2" t="n"><v>7</v></c><c r="C2"><f>B2*2</f><v>14</v></c><c r="D2" s="1" t="inlineStr"><is><t>&#48337;&#54633;</t></is></c></row>
  </sheetData>
  <mergeCells count="1"><mergeCell ref="D2:E3"/></mergeCells>
</worksheet>"#;

const SHEET_TWO: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>&#46160;&#48264;&#51704; &#49884;&#53944;</t></is></c></row></sheetData></worksheet>"#;

fn workbook_xml(sheet_name: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets><sheet name="{sheet_name}" sheetId="1" r:id="rId1"/><sheet name="두번째" sheetId="2" r:id="rId2"/></sheets>
</workbook>"#
    )
}

fn fixture(extra: &[(&str, &[u8])], sheet_name: &str) -> Vec<u8> {
    let cursor = Cursor::new(Vec::new());
    let mut archive = ZipWriter::new(cursor);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, body) in [
        ("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        ("_rels/.rels", ROOT_RELS.as_bytes()),
        ("xl/workbook.xml", workbook_xml(sheet_name).as_bytes()),
        ("xl/_rels/workbook.xml.rels", WORKBOOK_RELS.as_bytes()),
        ("xl/styles.xml", STYLES.as_bytes()),
        ("xl/worksheets/sheet1.xml", SHEET_ONE.as_bytes()),
        ("xl/worksheets/sheet2.xml", SHEET_TWO.as_bytes()),
    ] {
        archive.start_file(name, options).unwrap();
        archive.write_all(body).unwrap();
    }
    for (name, body) in extra {
        archive.start_file(*name, options).unwrap();
        archive.write_all(body).unwrap();
    }
    archive.finish().unwrap().into_inner()
}

fn part(bytes: &[u8], name: &str) -> Vec<u8> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut file = archive.by_name(name).unwrap();
    let mut body = Vec::new();
    file.read_to_end(&mut body).unwrap();
    body
}

fn headers() -> Vec<HeaderExpectation> {
    vec![
        HeaderExpectation {
            column: "A".to_owned(),
            expected: "WBS 코드".to_owned(),
        },
        HeaderExpectation {
            column: "B".to_owned(),
            expected: "작업명".to_owned(),
        },
    ]
}

#[test]
fn text_nodes_reads_only_text_and_cdata_inside_the_target() {
    let events = parse_xml_events(
            b"<root>outside<![CDATA[outside-cdata]]><t>inside<![CDATA[inside-cdata]]></t>tail<![CDATA[tail-cdata]]></root>",
            "text-nodes-test",
        )
        .unwrap();

    assert_eq!(text_nodes(&events, "t").unwrap(), "insideinside-cdata");
}

#[test]
fn surgical_golden_preserves_untouched_parts_styles_formulas_merges_and_hangul() {
    let input = fixture(&[], "데이터");
    let original_styles = part(&input, "xl/styles.xml");
    let original_second_sheet = part(&input, "xl/worksheets/sheet2.xml");
    let original_workbook = part(&input, "xl/workbook.xml");
    let formula = rollup_sum_formula("F", &[3, 4]).unwrap();
    let updates = vec![
        CellUpdate {
            coordinate: "A2".to_owned(),
            value: CellValue::Text("한글만".to_owned()),
        },
        CellUpdate {
            coordinate: "B2".to_owned(),
            value: CellValue::Number(12.5),
        },
        CellUpdate {
            coordinate: "A3".to_owned(),
            value: CellValue::Text("mixed 한글".to_owned()),
        },
        CellUpdate {
            coordinate: "C3".to_owned(),
            value: CellValue::Text("=CMD()".to_owned()),
        },
        CellUpdate {
            coordinate: "F2".to_owned(),
            value: CellValue::Formula(formula),
        },
    ];

    let XlsxFillOutcome::Written {
        bytes,
        escaped_cell_count,
    } = fill_workbook(&input, "데이터", &headers(), &updates).unwrap()
    else {
        panic!("golden must produce a workbook");
    };

    assert_eq!(escaped_cell_count, 1);
    assert_eq!(part(&bytes, "xl/styles.xml"), original_styles);
    assert_eq!(
        part(&bytes, "xl/worksheets/sheet2.xml"),
        original_second_sheet
    );
    assert_eq!(part(&bytes, "xl/workbook.xml"), original_workbook);

    let sheet = String::from_utf8(part(&bytes, "xl/worksheets/sheet1.xml")).unwrap();
    assert!(sheet.contains(r#"r="A2" s="1" t="inlineStr""#));
    assert!(sheet.contains("한글만"));
    assert!(sheet.contains("mixed 한글"));
    assert!(sheet.contains("12.5"));
    assert!(sheet.contains("<f>B2*2</f>"));
    assert!(sheet.contains("<f>SUM(F3,F4)</f>"));
    assert!(sheet.contains(r#"mergeCell ref="D2:E3""#));
    assert!(sheet.contains("&apos;=CMD()"));
    assert!(!sheet.contains("<f>CMD()</f>"));
}

#[test]
fn structure_hash_ignores_data_values_but_detects_header_change() {
    let input = fixture(&[], "데이터");
    let baseline = template_structure_hash(&input).unwrap();
    let XlsxFillOutcome::Written {
        bytes: data_edit, ..
    } = fill_workbook(
        &input,
        "데이터",
        &headers(),
        &[CellUpdate {
            coordinate: "A2".to_owned(),
            value: CellValue::Text("new data".to_owned()),
        }],
    )
    .unwrap()
    else {
        panic!("data edit must produce a workbook")
    };
    assert_eq!(template_structure_hash(&data_edit).unwrap(), baseline);

    let XlsxFillOutcome::Written {
        bytes: header_edit, ..
    } = fill_workbook(
        &input,
        "데이터",
        &headers(),
        &[CellUpdate {
            coordinate: "B1".to_owned(),
            value: CellValue::Text("변경된 헤더".to_owned()),
        }],
    )
    .unwrap()
    else {
        panic!("header edit must produce a workbook")
    };
    assert_ne!(template_structure_hash(&header_edit).unwrap(), baseline);
}

#[test]
fn header_drift_returns_no_artifact_before_updates() {
    let input = fixture(&[], "데이터");
    let mut expected = headers();
    expected[1].expected = "다른 헤더".to_owned();

    let outcome = fill_workbook(
        &input,
        "데이터",
        &expected,
        &[CellUpdate {
            coordinate: "A2".to_owned(),
            value: CellValue::Text("must not be written".to_owned()),
        }],
    )
    .unwrap();

    assert_eq!(
        outcome,
        XlsxFillOutcome::HeaderDrift(HeaderDrift {
            expected: vec!["WBS 코드".to_owned(), "다른 헤더".to_owned()],
            actual: vec!["WBS 코드".to_owned(), "작업명".to_owned()],
            first_mismatch_column: "B".to_owned(),
        })
    );
    assert_eq!(
        part(&input, "xl/worksheets/sheet1.xml"),
        SHEET_ONE.as_bytes()
    );
}

#[test]
fn sheet_lookup_compares_nfc_and_nfd_names() {
    let input = fixture(&[], "데이터");
    let outcome = fill_workbook(&input, "데이터", &headers(), &[]).unwrap();
    assert!(matches!(outcome, XlsxFillOutcome::Written { .. }));
}

#[test]
fn unsafe_package_parts_are_rejected() {
    let input = fixture(
        &[("xl/externalLinks/externalLink1.xml", b"<externalLink/>")],
        "데이터",
    );
    let error = fill_workbook(&input, "데이터", &headers(), &[]).unwrap_err();
    assert!(matches!(error, XlsxWriterError::UnsafeWorkbookPart(_)));
}

#[test]
fn unsafe_formulas_and_duplicate_updates_fail_closed() {
    let input = fixture(&[], "데이터");
    let unsafe_formula = fill_workbook(
        &input,
        "데이터",
        &headers(),
        &[CellUpdate {
            coordinate: "A2".to_owned(),
            value: CellValue::Formula("HYPERLINK(\"https://example.com\")".to_owned()),
        }],
    )
    .unwrap_err();
    assert!(matches!(unsafe_formula, XlsxWriterError::UnsafeFormula(_)));

    let duplicate = fill_workbook(
        &input,
        "데이터",
        &headers(),
        &[
            CellUpdate {
                coordinate: "a2".to_owned(),
                value: CellValue::Text("one".to_owned()),
            },
            CellUpdate {
                coordinate: "A2".to_owned(),
                value: CellValue::Text("two".to_owned()),
            },
        ],
    )
    .unwrap_err();
    assert!(matches!(duplicate, XlsxWriterError::DuplicateCellUpdate(_)));
}

#[test]
fn compressed_size_gate_runs_before_zip_parsing() {
    let oversized = vec![0_u8; MAX_TEMPLATE_BYTES + 1];
    let error = fill_workbook(&oversized, "데이터", &headers(), &[]).unwrap_err();
    assert!(matches!(error, XlsxWriterError::TemplateTooLarge { .. }));
}

#[test]
fn atomic_persist_replaces_target_without_leaving_a_sibling_temp_file() {
    let directory = tempdir().unwrap();
    let target = directory.path().join("output.xlsx");
    fs::write(&target, b"old").unwrap();

    persist_atomic(&target, b"new workbook").unwrap();

    assert_eq!(fs::read(&target).unwrap(), b"new workbook");
    let siblings = fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(siblings, vec![target.file_name().unwrap()]);
}

/// Rewrite one member's declared uncompressed size in its local header and
/// central directory record. The compressed stream and CRC stay intact, so
/// the member still decompresses to its real length.
fn understate_uncompressed_size(bytes: &mut [u8], member: &str, declared: u32) {
    let name = member.as_bytes();
    let mut patched = 0;
    let mut offset = 0;
    while offset + 4 <= bytes.len() {
        let signature = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let (size_field, name_length_field, name_start) = match signature {
            0x0403_4b50 => (22, 26, 30),
            0x0201_4b50 => (24, 28, 46),
            _ => {
                offset += 1;
                continue;
            }
        };
        let length_bytes = &bytes[offset + name_length_field..offset + name_length_field + 2];
        let name_length = usize::from(u16::from_le_bytes(length_bytes.try_into().unwrap()));
        let start = offset + name_start;
        if bytes.get(start..start + name_length) == Some(name) {
            bytes[offset + size_field..offset + size_field + 4]
                .copy_from_slice(&declared.to_le_bytes());
            patched += 1;
        }
        offset += 4;
    }
    assert_eq!(
        patched, 2,
        "expected one local header and one central record"
    );
}

#[test]
fn part_reads_enforce_declared_size_and_application_limit() {
    // Keep the application limit even when the ZIP declaration is truthful.
    let oversized = vec![b' '; MAX_XML_PART_BYTES as usize + 1];
    let mut input = fixture(&[(SHARED_STRINGS_PART, &oversized)], "데이터");
    let hash_error = template_structure_hash(&input).unwrap_err();
    assert!(
        matches!(
            hash_error,
            XlsxWriterError::DecompressedTooLarge { limit, .. } if limit == MAX_XML_PART_BYTES
        ),
        "{hash_error:?}"
    );
    let fill_error = fill_workbook(&input, "데이터", &headers(), &[]).unwrap_err();
    assert!(
        matches!(
            fill_error,
            XlsxWriterError::DecompressedTooLarge { limit, .. } if limit == MAX_XML_PART_BYTES
        ),
        "{fill_error:?}"
    );

    // ZIP 9 rejects an understated stream before the application byte cap.
    // Pin that stronger error without replacing the truthful-size cap above.
    understate_uncompressed_size(&mut input, SHARED_STRINGS_PART, 64);
    let mut archive = ZipArchive::new(Cursor::new(input.as_slice())).unwrap();
    assert_eq!(archive.by_name(SHARED_STRINGS_PART).unwrap().size(), 64);
    let hash_error = template_structure_hash(&input).unwrap_err();
    assert!(
        matches!(&hash_error, XlsxWriterError::Io(error) if error.kind() == std::io::ErrorKind::InvalidData),
        "{hash_error:?}"
    );
    let fill_error = fill_workbook(&input, "데이터", &headers(), &[]).unwrap_err();
    assert!(
        matches!(&fill_error, XlsxWriterError::Io(error) if error.kind() == std::io::ErrorKind::InvalidData),
        "{fill_error:?}"
    );
}

#[test]
fn surgical_copy_preserves_uid_gid_extra_field_on_untouched_member() {
    const EXTRA: [u8; 9] = [0x75, 0x78, 5, 0, 1, 1, 1, 1, 1];
    const MEMBER: &str = "docProps/custom.xml";
    const BODY: &[u8] = b"<Properties/>";
    let mut writer = ZipWriter::new_append(Cursor::new(fixture(&[], "데이터"))).unwrap();
    let mut options =
        zip::write::FullFileOptions::default().compression_method(CompressionMethod::Deflated);
    // Construct the fixture with an unrestricted vendor tag, then rewrite the
    // tag in both headers. This keeps fixture construction independent of the
    // unreserved feature whose read/raw-copy preservation is under test.
    options.add_extra_field(0xffff, &EXTRA[4..], false).unwrap();
    writer.start_file(MEMBER, options).unwrap();
    writer.write_all(BODY).unwrap();
    let mut input = writer.finish().unwrap().into_inner();
    let mut patched = 0;
    for offset in 0..input.len().saturating_sub(EXTRA.len() - 1) {
        if input[offset..offset + 2] == [0xff, 0xff]
            && input[offset + 2..offset + EXTRA.len()] == EXTRA[2..]
        {
            input[offset..offset + 2].copy_from_slice(&EXTRA[..2]);
            patched += 1;
        }
    }
    assert_eq!(patched, 2, "expected local and central UID/GID fields");
    let original_extra = ZipArchive::new(Cursor::new(&input))
        .unwrap()
        .by_name(MEMBER)
        .unwrap()
        .extra_data()
        .unwrap();
    assert!(original_extra
        .windows(EXTRA.len())
        .any(|field| field == EXTRA));
    let update = CellUpdate {
        coordinate: "A2".to_owned(),
        value: CellValue::Text("changed".to_owned()),
    };
    let XlsxFillOutcome::Written { bytes, .. } =
        fill_workbook(&input, "데이터", &headers(), &[update]).unwrap()
    else {
        panic!("matching headers must produce a workbook");
    };
    assert_eq!(part(&bytes, MEMBER), BODY);
    assert_ne!(
        part(&bytes, "xl/worksheets/sheet1.xml"),
        part(&input, "xl/worksheets/sheet1.xml")
    );
    assert_eq!(
        ZipArchive::new(Cursor::new(&bytes))
            .unwrap()
            .by_name(MEMBER)
            .unwrap()
            .extra_data()
            .unwrap(),
        original_extra
    );
}

/// Package whose workbook lists `sheet_count` sheets that all point at the one
/// worksheet part `worksheet`.
fn aliased_sheet_template(sheet_count: usize, worksheet: &str) -> Vec<u8> {
    let aliases = (0..sheet_count)
        .map(|index| {
            format!(
                r#"<sheet name="alias{index}" sheetId="{}" r:id="rId1"/>"#,
                index + 1
            )
        })
        .collect::<String>();
    let workbook = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>{aliases}</sheets></workbook>"#
    );
    let cursor = Cursor::new(Vec::new());
    let mut archive = ZipWriter::new(cursor);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, body) in [
        ("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        ("_rels/.rels", ROOT_RELS.as_bytes()),
        ("xl/workbook.xml", workbook.as_bytes()),
        ("xl/_rels/workbook.xml.rels", WORKBOOK_RELS.as_bytes()),
        ("xl/styles.xml", STYLES.as_bytes()),
        ("xl/worksheets/sheet1.xml", worksheet.as_bytes()),
    ] {
        archive.start_file(name, options).unwrap();
        archive.write_all(body).unwrap();
    }
    archive.finish().unwrap().into_inner()
}

#[test]
fn repeated_reads_of_an_aliased_worksheet_share_one_decompression_budget() {
    // Six sheet entries alias one 20 MiB worksheet. Each read is within the part
    // limit and the archive's declared total is honest, but the reads add up to
    // 120 MiB of decompression in one call.
    let mut sheet = String::from(
        r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#,
    );
    sheet.push_str(&" ".repeat(20 * 1024 * 1024));
    sheet.push_str("</sheetData></worksheet>");
    let input = aliased_sheet_template(6, &sheet);

    let error = template_structure_hash(&input).unwrap_err();
    assert!(
        matches!(
            error,
            XlsxWriterError::DecompressedTooLarge { limit, .. } if limit == MAX_DECOMPRESSED_BYTES
        ),
        "{error:?}"
    );
}

#[test]
fn workbooks_listing_more_sheets_than_archive_entries_are_rejected() {
    // Hashing rescans workbook.xml once per sheet. Before this cap, 16,000
    // aliased sheets in a 73 KB template took 5.5 minutes in a debug build, and
    // the time quadrupled with every doubling of the sheet count.
    let input = aliased_sheet_template(MAX_ZIP_ENTRIES + 1, SHEET_TWO);
    let error = template_structure_hash(&input).unwrap_err();
    assert!(
        matches!(&error, XlsxWriterError::Malformed(message) if message.contains("worksheets")),
        "{error:?}"
    );

    let accepted = template_structure_hash(&aliased_sheet_template(2, SHEET_TWO)).unwrap();
    assert_eq!(accepted.len(), 64);
}

#[test]
fn template_reads_stop_at_the_size_gate() {
    let directory = tempdir().unwrap();
    let fitting = directory.path().join("fitting.xlsx");
    let input = fixture(&[], "데이터");
    fs::write(&fitting, &input).unwrap();
    assert_eq!(read_template_file(&fitting).unwrap(), input);

    // Exactly at the limit is still accepted: both the metadata gate and the bounded
    // read reject only what exceeds the limit.
    let at_limit = directory.path().join("at-limit.xlsx");
    fs::write(&at_limit, vec![0_u8; MAX_TEMPLATE_BYTES]).unwrap();
    assert_eq!(
        read_template_file(&at_limit).unwrap().len(),
        MAX_TEMPLATE_BYTES
    );

    // The metadata length rejects the file before any read: `actual` is the full
    // length, not the bounded read's limit + 1.
    let oversized = directory.path().join("oversized.xlsx");
    fs::write(&oversized, vec![0_u8; MAX_TEMPLATE_BYTES + 10]).unwrap();
    let error = read_template_file(&oversized).unwrap_err();
    assert!(
        matches!(error, XlsxWriterError::TemplateTooLarge { actual, limit }
            if actual == MAX_TEMPLATE_BYTES + 10 && limit == MAX_TEMPLATE_BYTES),
        "{error:?}"
    );
}

#[cfg(unix)]
#[test]
fn endless_template_input_stops_one_byte_past_the_gate() {
    // A device reports length 0 and never ends; only the bounded read stops it.
    let error = read_template_file(Path::new("/dev/zero")).unwrap_err();
    assert!(
        matches!(error, XlsxWriterError::TemplateTooLarge { actual, limit }
            if actual == MAX_TEMPLATE_BYTES + 1 && limit == MAX_TEMPLATE_BYTES),
        "{error:?}"
    );
}
