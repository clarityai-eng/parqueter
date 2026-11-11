use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::schema::types::Type;
use std::fs::File;
use std::path::Path;

pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
}

pub fn read_schema(path: &Path) -> Result<Vec<ColumnInfo>, String> {
    let file = File::open(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("File not found: {}", path.display()),
        std::io::ErrorKind::PermissionDenied => {
            format!("Permission denied: {}", path.display())
        }
        _ => format!("Cannot open file: {e}"),
    })?;

    let reader = SerializedFileReader::new(file)
        .map_err(|e| format!("Not a valid Parquet file: {e}"))?;

    let schema = reader.metadata().file_metadata().schema();
    let columns = schema
        .get_fields()
        .iter()
        .map(|field| ColumnInfo {
            name: field.name().to_string(),
            data_type: format_type(field),
        })
        .collect();

    Ok(columns)
}

fn format_type(field: &Type) -> String {
    match field {
        Type::PrimitiveType {
            basic_info,
            physical_type,
            ..
        } => {
            if let Some(logical) = basic_info.logical_type_ref() {
                format!("{logical:?}")
            } else if basic_info.converted_type()
                != parquet::basic::ConvertedType::NONE
            {
                format!("{}", basic_info.converted_type())
            } else {
                format!("{physical_type}")
            }
        }
        Type::GroupType { .. } => "GROUP".to_string(),
    }
}
