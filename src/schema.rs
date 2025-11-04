use rmcp::model::JsonObject;
use rmcp::schemars::{self, generate::SchemaSettings, JsonSchema};
use std::sync::Arc;

/// Generate an inline JSON schema for MCP tools
///
/// Unlike rmcp's default `schema_for_type()`, this function sets `inline_subschemas = true`
/// to generate inline enum definitions instead of $ref patterns. This ensures MCP Inspector
/// displays enums as dropdown widgets rather than raw JSON input fields.
pub fn inline_schema_for_type<T: JsonSchema>() -> Arc<JsonObject> {
    let mut settings = SchemaSettings::draft07();
    settings.transforms = vec![Box::new(schemars::transform::AddNullable::default())];
    settings.inline_subschemas = true;

    let generator = settings.into_generator();
    let schema = generator.into_root_schema_for::<T>();
    let object = serde_json::to_value(schema).expect("failed to serialize schema");

    let json_object = match object {
        serde_json::Value::Object(object) => object,
        _ => panic!("Schema serialization produced non-object value"),
    };

    Arc::new(json_object)
}
