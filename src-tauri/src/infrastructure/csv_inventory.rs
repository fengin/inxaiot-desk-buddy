use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;

use csv::{ReaderBuilder, StringRecord, Trim};

use crate::core::error::{AppError, AppResult};
use crate::domain::aio::inventory::{InventoryValues, ParsedInventoryRow, validate_inventory_values};

const MAX_INVENTORY_BYTES: u64 = 10 * 1024 * 1024;
const REQUIRED_HEADERS: &[&str] = &["name", "ip", "mac"];
const OPTIONAL_HEADERS: &[&str] = &[
    "buildingId",
    "regionId",
    "addrAlias",
    "floor",
    "location",
    "remark",
];

pub fn parse_inventory_path(path: &Path) -> AppResult<Vec<ParsedInventoryRow>> {
    let metadata =
        std::fs::metadata(path).map_err(|error| AppError::io("读取一体机清单属性", &error))?;
    if metadata.len() > MAX_INVENTORY_BYTES {
        return Err(AppError::InvalidConfig("一体机清单不能超过 10 MiB".into()));
    }
    let bytes = std::fs::read(path).map_err(|error| AppError::io("读取一体机清单文件", &error))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| AppError::InvalidConfig("一体机清单必须使用 UTF-8 编码".into()))?;
    parse_inventory_text(&text)
}

pub fn parse_inventory_text(text: &str) -> AppResult<Vec<ParsedInventoryRow>> {
    let text = text.strip_prefix('﻿').unwrap_or(text);
    let mut reader = ReaderBuilder::new()
        .trim(Trim::All)
        .flexible(true)
        .from_reader(Cursor::new(text.as_bytes()));
    let headers = reader
        .headers()
        .map_err(|error| invalid_csv("读取清单表头", &error))?
        .clone();
    let header_index = build_header_index(&headers)?;
    let mut rows = Vec::new();
    for (index, record) in reader.records().enumerate() {
        let row_number = u32::try_from(index + 2).unwrap_or(u32::MAX);
        match record {
            Ok(record) if record.iter().all(|value| value.trim().is_empty()) => continue,
            Ok(record) => rows.push(parse_row(row_number, &record, &header_index)),
            Err(error) => rows.push(ParsedInventoryRow {
                row_number,
                values: InventoryValues::default(),
                mac_normalized: None,
                errors: vec![format!("第 {row_number} 行：CSV 格式错误（{error}）")],
            }),
        }
    }
    if rows.is_empty() {
        return Err(AppError::InvalidConfig("一体机清单至少需要一行数据".into()));
    }
    Ok(rows)
}

fn build_header_index(headers: &StringRecord) -> AppResult<HashMap<String, usize>> {
    let mut index = HashMap::new();
    for (position, raw) in headers.iter().enumerate() {
        let name = raw.trim().trim_start_matches('﻿').to_string();
        if name.is_empty() {
            continue;
        }
        if index.insert(name.clone(), position).is_some() {
            return Err(AppError::InvalidConfig(format!(
                "一体机清单存在重复列：{name}"
            )));
        }
    }
    let missing = REQUIRED_HEADERS
        .iter()
        .filter(|name| !index.contains_key(**name))
        .copied()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(AppError::InvalidConfig(format!(
            "一体机清单缺少必填列：{}",
            missing.join("、")
        )));
    }
    let supported = REQUIRED_HEADERS
        .iter()
        .chain(OPTIONAL_HEADERS)
        .copied()
        .collect::<Vec<_>>();
    let unknown = index
        .keys()
        .filter(|name| !supported.contains(&name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        tracing::warn!(
            column_count = unknown.len(),
            "inventory contains ignored columns"
        );
    }
    Ok(index)
}

fn parse_row(
    row_number: u32,
    record: &StringRecord,
    headers: &HashMap<String, usize>,
) -> ParsedInventoryRow {
    let values = InventoryValues {
        name: value(record, headers, "name"),
        ip: value(record, headers, "ip"),
        mac: value(record, headers, "mac"),
        building_id: optional_value(record, headers, "buildingId"),
        region_id: optional_value(record, headers, "regionId"),
        addr_alias: optional_value(record, headers, "addrAlias"),
        floor: optional_value(record, headers, "floor"),
        location: optional_value(record, headers, "location"),
        remark: optional_value(record, headers, "remark"),
    };
    validate_inventory_values(row_number, values)
}

fn value(record: &StringRecord, headers: &HashMap<String, usize>, name: &str) -> String {
    headers
        .get(name)
        .and_then(|index| record.get(*index))
        .unwrap_or("")
        .trim()
        .to_string()
}

fn optional_value(
    record: &StringRecord,
    headers: &HashMap<String, usize>,
    name: &str,
) -> Option<String> {
    let value = value(record, headers, name);
    (!value.is_empty()).then_some(value)
}

fn invalid_csv(operation: &'static str, error: &csv::Error) -> AppError {
    tracing::warn!(operation, error = ?crate::core::log_safety::safe_error(error), "invalid inventory csv");
    AppError::InvalidConfig(format!("{operation}失败：CSV 格式无效"))
}

#[cfg(test)]
mod tests {
    use super::parse_inventory_text;

    #[test]
    fn csv_parser_supports_bom_quotes_optional_fields_and_empty_lines() {
        let text = format!(
            "﻿{}",
            r#"name,ip,mac,location,remark
"node, one",192.168.3.101,AA-BB-CC-DD-EE-01,"A,1F",测试
,,,,
"#
        );
        let rows = parse_inventory_text(&text).expect("parse CSV");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].values.name, "node, one");
        assert_eq!(rows[0].values.location.as_deref(), Some("A,1F"));
        assert_eq!(rows[0].mac_normalized.as_deref(), Some("AABBCCDDEE01"));
        assert!(rows[0].errors.is_empty());
    }

    #[test]
    fn csv_parser_reports_row_validation_and_global_header_errors() {
        let rows = parse_inventory_text("name,ip,mac\n,invalid,not-a-mac\n").expect("parse rows");
        assert_eq!(rows[0].errors.len(), 3);
        assert!(parse_inventory_text("name,ip\nnode,192.168.3.1\n").is_err());
        assert!(parse_inventory_text("name,ip,mac\n").is_err());
    }
}
