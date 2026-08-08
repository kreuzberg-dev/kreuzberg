//! PDF form field extraction using the pdf_oxide backend.
//!
//! Maps pdf_oxide's `FormField` types to Xberg's `PdfFormField` model,
//! extracting field names, types, values, bounding boxes, and metadata.
//!
//! Supports both **AcroForm and XFA forms**. When a document contains both, AcroForm
//! fields are extracted first (canonical layer per PDF spec), and XFA-only fields are
//! appended. XFA-only forms are fully extracted.

use super::OxideDocument;
use crate::types::{BoundingBox, FormFieldType, PdfFormField, ProcessingWarning};

/// Extract form fields from a PDF document using pdf_oxide.
///
/// Extracts from both AcroForm and XFA layers:
/// 1. AcroForm extraction via `FormExtractor::extract_fields`
/// 2. XFA extraction via the `pdf_oxide::xfa` parser
///
/// When both layers are present, AcroForm fields take priority (canonical layer per PDF spec);
/// only XFA fields whose names are not already present in the AcroForm set are appended, so
/// hybrid documents (which mirror the same fields in both layers) are not double-counted.
/// Extraction failures are logged but do not prevent processing.
///
/// # Arguments
///
/// * `doc` - The opened PDF document (mutable: XFA object resolution requires it)
///
/// # Returns
///
/// A `Vec<PdfFormField>` containing all successfully extracted form fields from both layers,
/// or an empty vector if neither AcroForm nor XFA is present or extraction fails completely.
/// The accompanying `Vec<ProcessingWarning>` surfaces AcroForm/XFA extraction or parsing
/// failures (issue #73) that would otherwise make a form-bearing PDF indistinguishable from
/// one with no form at all. A document with no AcroForm and no XFA (the common case) produces
/// no warnings — only actual failures are reported.
pub(crate) fn extract_form_fields(doc: &mut OxideDocument) -> (Vec<PdfFormField>, Vec<ProcessingWarning>) {
    let mut fields = Vec::new();
    let mut warnings = Vec::new();

    match pdf_oxide::extractors::forms::FormExtractor::extract_fields(&doc.doc) {
        Ok(oxide_fields) => {
            fields.extend(oxide_fields.into_iter().map(map_form_field));
            tracing::debug!("extracted {} AcroForm fields", fields.len());
        }
        Err(e) => {
            // `extract_fields` returns `Ok(vec![])` when the document simply has no
            // `/AcroForm` entry, so any `Err` here is a genuine parse/extraction
            // failure, not the (very common) absence of a form.
            tracing::debug!("AcroForm extraction failed: {e}");
            warnings.push(ProcessingWarning {
                source: std::borrow::Cow::Borrowed("pdf_forms"),
                message: std::borrow::Cow::Owned(format!("AcroForm field extraction failed: {e}")),
            });
        }
    }

    match extract_xfa_fields(doc) {
        Ok(Some(xfa_fields)) => {
            let known: std::collections::HashSet<String> = fields.iter().map(|f| f.name.clone()).collect();
            let appended: Vec<PdfFormField> = xfa_fields.into_iter().filter(|f| !known.contains(&f.name)).collect();
            if !appended.is_empty() {
                tracing::debug!("appended {} XFA-only fields", appended.len());
                fields.extend(appended);
            }
        }
        Ok(None) => {}
        Err(warning) => warnings.push(warning),
    }

    (fields, warnings)
}

/// pdf_oxide reports "no AcroForm"/"no XFA entry" as an `Err(Error::InvalidPdf(..))` with
/// one of these exact messages — the overwhelmingly common case, since most PDFs have no
/// XFA layer at all. Any other error means XFA was present but malformed/undecodable.
const XFA_ABSENT_MESSAGES: [&str; 2] = ["No AcroForm in document", "No XFA entry in AcroForm"];

/// Extract form fields from XFA layer (if present).
///
/// Attempts to detect, parse, and convert XFA form data using pdf_oxide's XFA module.
/// Returns `Ok(None)` if XFA is not present. Returns `Err(warning)` if XFA is present but
/// extraction or parsing failed (issue #73), so the caller can surface the failure instead
/// of silently treating it the same as "no XFA form".
fn extract_xfa_fields(doc: &mut OxideDocument) -> Result<Option<Vec<PdfFormField>>, ProcessingWarning> {
    let xfa_data = match pdf_oxide::xfa::XfaExtractor::extract_xfa(&mut doc.doc) {
        Ok(data) => data,
        Err(e) => {
            let message = e.to_string();
            if XFA_ABSENT_MESSAGES.iter().any(|absent| message.contains(absent)) {
                tracing::debug!("XFA not present: {e}");
                return Ok(None);
            }
            tracing::debug!("XFA extraction failed: {e}");
            return Err(ProcessingWarning {
                source: std::borrow::Cow::Borrowed("pdf_forms"),
                message: std::borrow::Cow::Owned(format!(
                    "XFA form data was present but could not be extracted: {e}; XFA fields were skipped"
                )),
            });
        }
    };

    let mut parser = pdf_oxide::xfa::XfaParser::new();
    let xfa_form = match parser.parse(&xfa_data) {
        Ok(form) => form,
        Err(e) => {
            tracing::debug!("XFA parsing failed: {e}");
            return Err(ProcessingWarning {
                source: std::borrow::Cow::Borrowed("pdf_forms"),
                message: std::borrow::Cow::Owned(format!(
                    "XFA form data was present but could not be parsed: {e}; XFA fields were skipped"
                )),
            });
        }
    };

    let mut fields = Vec::new();

    for xfa_field in &xfa_form.fields {
        let field = map_xfa_field_direct(xfa_field);
        fields.push(field);
    }

    Ok(Some(fields))
}

/// Maps a single pdf_oxide FormField to a Xberg PdfFormField.
///
/// Converts all field properties including type, value, bounds, and metadata.
/// Type mapping and value extraction handle various field subtypes (text, checkbox, radio, choice, signature, button).
fn map_form_field(oxide_field: pdf_oxide::extractors::forms::FormField) -> PdfFormField {
    let field_type = map_field_type(&oxide_field.field_type, oxide_field.flags);
    let value = map_field_value(&oxide_field.value);
    let default_value = oxide_field.default_value.as_ref().and_then(map_field_value);
    let bbox = oxide_field.bounds.map(|bounds| BoundingBox {
        x0: bounds[0],
        y0: bounds[1],
        x1: bounds[2],
        y1: bounds[3],
    });

    PdfFormField {
        name: oxide_field.name,
        full_name: oxide_field.full_name,
        field_type,
        value,
        default_value,
        flags: oxide_field.flags.unwrap_or(0),
        page: None,
        bbox,
        max_length: oxide_field.max_length,
        tooltip: oxide_field.tooltip,
    }
}

/// Maps pdf_oxide's `FieldType` to Xberg's `FormFieldType`.
///
/// Matches the PDF form field type hierarchy:
/// - Button (/Btn) includes checkboxes, radio buttons, and push buttons
/// - Text (/Tx) is single- or multi-line text input
/// - Choice (/Ch) is dropdown or list box
/// - Signature (/Sig) is digital signature
/// - Unknown for unrecognized types
///
/// For Button fields, further classifies using field flags per ISO 32000-1 §12.7.4.2:
/// - PUSH_BUTTON (bit 16): returns `Button`
/// - RADIO (bit 15): returns `Radio`
/// - Neither: returns `Checkbox` (default button subtype)
fn map_field_type(oxide_type: &pdf_oxide::extractors::forms::FieldType, flags: Option<u32>) -> FormFieldType {
    use pdf_oxide::extractors::forms::{FieldType, field_flags};
    match oxide_type {
        FieldType::Button => {
            if let Some(f) = flags {
                if f & field_flags::PUSH_BUTTON != 0 {
                    FormFieldType::Button
                } else if f & field_flags::RADIO != 0 {
                    FormFieldType::Radio
                } else {
                    FormFieldType::Checkbox
                }
            } else {
                FormFieldType::Checkbox
            }
        }
        FieldType::Text => FormFieldType::Text,
        FieldType::Choice => FormFieldType::Choice,
        FieldType::Signature => FormFieldType::Signature,
        FieldType::Unknown(_) => FormFieldType::Unknown,
    }
}

/// Maps pdf_oxide's `FieldValue` to a String representation.
///
/// Handles:
/// - Text: returned as-is
/// - Boolean: converted to "true" or "false" (e.g., checkbox Boolean(true) → "true")
/// - Name: value returned as-is (e.g., radio Name("Opt") → "Opt", dropdown selections)
/// - Array: values joined with ", " (multi-select list boxes)
/// - None: returns None
fn map_field_value(oxide_value: &pdf_oxide::extractors::forms::FieldValue) -> Option<String> {
    use pdf_oxide::extractors::forms::FieldValue;
    match oxide_value {
        FieldValue::Text(s) => Some(s.clone()),
        FieldValue::Boolean(b) => Some(b.to_string()),
        FieldValue::Name(n) => Some(n.clone()),
        FieldValue::Array(arr) => {
            if arr.is_empty() {
                None
            } else {
                Some(arr.join(", "))
            }
        }
        FieldValue::None => None,
    }
}

/// Maps a pdf_oxide XfaFieldType to Xberg's FormFieldType.
///
/// Handles the XFA field type taxonomy:
/// - Text/Numeric/DateTime → Text
/// - Checkbox → Checkbox
/// - RadioGroup → Radio
/// - DropDown/ListBox → Choice
/// - Signature → Signature
/// - Button → Button
/// - Everything else → Unknown
fn map_xfa_field_type(xfa_type: &pdf_oxide::xfa::XfaFieldType) -> FormFieldType {
    use pdf_oxide::xfa::XfaFieldType;
    match xfa_type {
        XfaFieldType::Text | XfaFieldType::Numeric | XfaFieldType::DateTime => FormFieldType::Text,
        XfaFieldType::Checkbox => FormFieldType::Checkbox,
        XfaFieldType::RadioGroup => FormFieldType::Radio,
        XfaFieldType::DropDown | XfaFieldType::ListBox => FormFieldType::Choice,
        XfaFieldType::Signature => FormFieldType::Signature,
        XfaFieldType::Button => FormFieldType::Button,
        _ => FormFieldType::Unknown,
    }
}

/// Maps a pdf_oxide XfaField (from XFA parsing) to a Xberg PdfFormField.
///
/// Extracts field type, name, value, and layout information from the parsed XFA field.
/// XFA fields contain all necessary metadata: name, field type, current value, default value,
/// dimensions, and tooltip/caption text.
fn map_xfa_field_direct(xfa_field: &pdf_oxide::xfa::XfaField) -> PdfFormField {
    let field_type = map_xfa_field_type(&xfa_field.field_type);

    let value = xfa_field.value.clone().or_else(|| xfa_field.default_value.clone());

    let bbox =
        if let (Some(x), Some(y), Some(w), Some(h)) = (xfa_field.x, xfa_field.y, xfa_field.width, xfa_field.height) {
            Some(BoundingBox {
                x0: x as f64,
                y0: y as f64,
                x1: (x + w) as f64,
                y1: (y + h) as f64,
            })
        } else {
            None
        };

    PdfFormField {
        name: xfa_field.name.clone(),
        full_name: xfa_field.binding.clone(),
        field_type,
        value,
        default_value: xfa_field.default_value.clone(),
        flags: 0,
        page: None,
        bbox,
        max_length: xfa_field.max_length,
        tooltip: xfa_field.tooltip.clone().or_else(|| xfa_field.caption.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_oxide::extractors::forms::{FieldType, FieldValue};

    #[test]
    fn test_map_field_type_button_push_button() {
        use pdf_oxide::extractors::forms::field_flags;
        let flags = field_flags::PUSH_BUTTON;
        assert_eq!(map_field_type(&FieldType::Button, Some(flags)), FormFieldType::Button);
    }

    #[test]
    fn test_map_field_type_button_radio() {
        use pdf_oxide::extractors::forms::field_flags;
        let flags = field_flags::RADIO;
        assert_eq!(map_field_type(&FieldType::Button, Some(flags)), FormFieldType::Radio);
    }

    #[test]
    fn test_map_field_type_button_checkbox_no_flags() {
        assert_eq!(map_field_type(&FieldType::Button, None), FormFieldType::Checkbox);
    }

    #[test]
    fn test_map_field_type_button_checkbox_default() {
        assert_eq!(map_field_type(&FieldType::Button, Some(0)), FormFieldType::Checkbox);
    }

    #[test]
    fn test_map_field_type_text() {
        assert_eq!(map_field_type(&FieldType::Text, None), FormFieldType::Text);
    }

    #[test]
    fn test_map_field_type_choice() {
        assert_eq!(map_field_type(&FieldType::Choice, None), FormFieldType::Choice);
    }

    #[test]
    fn test_map_field_type_signature() {
        assert_eq!(map_field_type(&FieldType::Signature, None), FormFieldType::Signature);
    }

    #[test]
    fn test_map_field_type_unknown() {
        assert_eq!(
            map_field_type(&FieldType::Unknown("custom".to_string()), None),
            FormFieldType::Unknown
        );
    }

    #[test]
    fn test_map_field_value_text() {
        let value = FieldValue::Text("hello".to_string());
        assert_eq!(map_field_value(&value), Some("hello".to_string()));
    }

    #[test]
    fn test_map_field_value_boolean_true() {
        let value = FieldValue::Boolean(true);
        assert_eq!(map_field_value(&value), Some("true".to_string()));
    }

    #[test]
    fn test_map_field_value_boolean_false() {
        let value = FieldValue::Boolean(false);
        assert_eq!(map_field_value(&value), Some("false".to_string()));
    }

    #[test]
    fn test_map_field_value_name() {
        let value = FieldValue::Name("Yes".to_string());
        assert_eq!(map_field_value(&value), Some("Yes".to_string()));
    }

    #[test]
    fn test_map_field_value_array() {
        let value = FieldValue::Array(vec!["option1".to_string(), "option2".to_string()]);
        assert_eq!(map_field_value(&value), Some("option1, option2".to_string()));
    }

    #[test]
    fn test_map_field_value_empty_array() {
        let value = FieldValue::Array(vec![]);
        assert_eq!(map_field_value(&value), None);
    }

    #[test]
    fn test_map_field_value_none() {
        let value = FieldValue::None;
        assert_eq!(map_field_value(&value), None);
    }

    #[test]
    fn test_checkbox_boolean_true_maps_to_true_string() {
        let value = FieldValue::Boolean(true);
        let mapped = map_field_value(&value).expect("Some");
        assert_eq!(mapped, "true", "checkbox Boolean(true) should map to 'true'");
    }

    #[test]
    fn test_radio_name_value_maps_as_is() {
        let value = FieldValue::Name("Opt".to_string());
        let mapped = map_field_value(&value).expect("Some");
        assert_eq!(mapped, "Opt", "radio Name('Opt') should map to 'Opt'");
    }

    #[test]
    fn test_checkbox_field_classification_and_value() {
        let field_type = map_field_type(&FieldType::Button, Some(0));
        let value = map_field_value(&FieldValue::Boolean(true));
        assert_eq!(field_type, FormFieldType::Checkbox);
        assert_eq!(value, Some("true".to_string()));
    }

    #[test]
    fn test_radio_field_classification_and_value() {
        use pdf_oxide::extractors::forms::field_flags;
        let field_type = map_field_type(&FieldType::Button, Some(field_flags::RADIO));
        let value = map_field_value(&FieldValue::Name("Choice".to_string()));
        assert_eq!(field_type, FormFieldType::Radio);
        assert_eq!(value, Some("Choice".to_string()));
    }

    #[test]
    fn test_push_button_field_classification() {
        use pdf_oxide::extractors::forms::field_flags;
        let field_type = map_field_type(&FieldType::Button, Some(field_flags::PUSH_BUTTON));
        assert_eq!(field_type, FormFieldType::Button);
    }

    #[test]
    fn test_map_xfa_field_type_text() {
        let xfa_type = pdf_oxide::xfa::XfaFieldType::Text;
        assert_eq!(map_xfa_field_type(&xfa_type), FormFieldType::Text);
    }

    #[test]
    fn test_map_xfa_field_type_numeric() {
        let xfa_type = pdf_oxide::xfa::XfaFieldType::Numeric;
        assert_eq!(map_xfa_field_type(&xfa_type), FormFieldType::Text);
    }

    #[test]
    fn test_map_xfa_field_type_datetime() {
        let xfa_type = pdf_oxide::xfa::XfaFieldType::DateTime;
        assert_eq!(map_xfa_field_type(&xfa_type), FormFieldType::Text);
    }

    #[test]
    fn test_map_xfa_field_type_checkbox() {
        let xfa_type = pdf_oxide::xfa::XfaFieldType::Checkbox;
        assert_eq!(map_xfa_field_type(&xfa_type), FormFieldType::Checkbox);
    }

    #[test]
    fn test_map_xfa_field_type_radio_group() {
        let xfa_type = pdf_oxide::xfa::XfaFieldType::RadioGroup;
        assert_eq!(map_xfa_field_type(&xfa_type), FormFieldType::Radio);
    }

    #[test]
    fn test_map_xfa_field_type_dropdown() {
        let xfa_type = pdf_oxide::xfa::XfaFieldType::DropDown;
        assert_eq!(map_xfa_field_type(&xfa_type), FormFieldType::Choice);
    }

    #[test]
    fn test_map_xfa_field_type_listbox() {
        let xfa_type = pdf_oxide::xfa::XfaFieldType::ListBox;
        assert_eq!(map_xfa_field_type(&xfa_type), FormFieldType::Choice);
    }

    #[test]
    fn test_map_xfa_field_type_signature() {
        let xfa_type = pdf_oxide::xfa::XfaFieldType::Signature;
        assert_eq!(map_xfa_field_type(&xfa_type), FormFieldType::Signature);
    }

    #[test]
    fn test_map_xfa_field_type_button() {
        let xfa_type = pdf_oxide::xfa::XfaFieldType::Button;
        assert_eq!(map_xfa_field_type(&xfa_type), FormFieldType::Button);
    }

    #[test]
    fn test_map_xfa_field_type_image() {
        let xfa_type = pdf_oxide::xfa::XfaFieldType::Image;
        assert_eq!(map_xfa_field_type(&xfa_type), FormFieldType::Unknown);
    }

    #[test]
    fn test_map_xfa_field_type_barcode() {
        let xfa_type = pdf_oxide::xfa::XfaFieldType::Barcode;
        assert_eq!(map_xfa_field_type(&xfa_type), FormFieldType::Unknown);
    }

    #[test]
    fn test_map_xfa_field_type_unknown() {
        let xfa_type = pdf_oxide::xfa::XfaFieldType::Unknown("CustomType".to_string());
        assert_eq!(map_xfa_field_type(&xfa_type), FormFieldType::Unknown);
    }

    #[test]
    fn test_map_xfa_field_direct_text_with_value() {
        let xfa_field = pdf_oxide::xfa::XfaField::new("username", "form.username[0]");
        let mut xfa_field = xfa_field;
        xfa_field.field_type = pdf_oxide::xfa::XfaFieldType::Text;
        xfa_field.value = Some("john_doe".to_string());

        let mapped = map_xfa_field_direct(&xfa_field);
        assert_eq!(mapped.name, "username");
        assert_eq!(mapped.full_name, "form.username[0]");
        assert_eq!(mapped.field_type, FormFieldType::Text);
        assert_eq!(mapped.value, Some("john_doe".to_string()));
    }

    #[test]
    fn test_map_xfa_field_direct_checkbox_checked() {
        let xfa_field = pdf_oxide::xfa::XfaField::new("agree", "form.agree[0]");
        let mut xfa_field = xfa_field;
        xfa_field.field_type = pdf_oxide::xfa::XfaFieldType::Checkbox;
        xfa_field.value = Some("Yes".to_string());

        let mapped = map_xfa_field_direct(&xfa_field);
        assert_eq!(mapped.name, "agree");
        assert_eq!(mapped.field_type, FormFieldType::Checkbox);
        assert_eq!(mapped.value, Some("Yes".to_string()));
    }

    #[test]
    fn test_map_xfa_field_direct_with_bbox() {
        let xfa_field = pdf_oxide::xfa::XfaField::new("name_field", "form.name[0]");
        let mut xfa_field = xfa_field;
        xfa_field.field_type = pdf_oxide::xfa::XfaFieldType::Text;
        xfa_field.x = Some(72.0);
        xfa_field.y = Some(700.0);
        xfa_field.width = Some(200.0);
        xfa_field.height = Some(20.0);

        let mapped = map_xfa_field_direct(&xfa_field);
        let bbox = mapped.bbox.expect("bbox should be present");
        assert_eq!(bbox.x0, 72.0);
        assert_eq!(bbox.y0, 700.0);
        assert_eq!(bbox.x1, 272.0);
        assert_eq!(bbox.y1, 720.0);
    }

    #[test]
    fn test_map_xfa_field_direct_with_tooltip() {
        let xfa_field = pdf_oxide::xfa::XfaField::new("email", "form.email[0]");
        let mut xfa_field = xfa_field;
        xfa_field.field_type = pdf_oxide::xfa::XfaFieldType::Text;
        xfa_field.tooltip = Some("Enter your email address".to_string());

        let mapped = map_xfa_field_direct(&xfa_field);
        assert_eq!(mapped.tooltip, Some("Enter your email address".to_string()));
    }

    #[test]
    fn test_map_xfa_field_direct_tooltip_fallback_to_caption() {
        let xfa_field = pdf_oxide::xfa::XfaField::new("phone", "form.phone[0]");
        let mut xfa_field = xfa_field;
        xfa_field.field_type = pdf_oxide::xfa::XfaFieldType::Text;
        xfa_field.caption = Some("Phone Number".to_string());

        let mapped = map_xfa_field_direct(&xfa_field);
        assert_eq!(mapped.tooltip, Some("Phone Number".to_string()));
    }

    #[test]
    fn test_map_xfa_field_direct_max_length() {
        let xfa_field = pdf_oxide::xfa::XfaField::new("zip", "form.zip[0]");
        let mut xfa_field = xfa_field;
        xfa_field.field_type = pdf_oxide::xfa::XfaFieldType::Numeric;
        xfa_field.max_length = Some(5);

        let mapped = map_xfa_field_direct(&xfa_field);
        assert_eq!(mapped.max_length, Some(5));
    }

    #[test]
    fn test_map_xfa_field_direct_value_prefers_current() {
        let xfa_field = pdf_oxide::xfa::XfaField::new("status", "form.status[0]");
        let mut xfa_field = xfa_field;
        xfa_field.field_type = pdf_oxide::xfa::XfaFieldType::Text;
        xfa_field.value = Some("Active".to_string());
        xfa_field.default_value = Some("Inactive".to_string());

        let mapped = map_xfa_field_direct(&xfa_field);
        assert_eq!(mapped.value, Some("Active".to_string()));
        assert_eq!(mapped.default_value, Some("Inactive".to_string()));
    }

    #[test]
    fn test_map_xfa_field_direct_value_fallback_to_default() {
        let xfa_field = pdf_oxide::xfa::XfaField::new("priority", "form.priority[0]");
        let mut xfa_field = xfa_field;
        xfa_field.field_type = pdf_oxide::xfa::XfaFieldType::Text;
        xfa_field.default_value = Some("Normal".to_string());

        let mapped = map_xfa_field_direct(&xfa_field);
        assert_eq!(mapped.value, Some("Normal".to_string()));
    }
}
