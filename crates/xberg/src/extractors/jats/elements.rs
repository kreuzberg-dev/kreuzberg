//! Element extraction (title, abstract, body, references, tables).

use super::metadata::JatsMetadataExtracted;
use super::parser::{extract_formula_latex, extract_text_content};
use crate::Result;
use crate::extraction::cells_to_markdown;
use crate::extractors::security::SecurityBudget;
use crate::types::Table;
use quick_xml::events::Event;

use crate::utils::xml_utils::EntityReader;

/// Extract all content in a single optimized pass.
/// Combines metadata extraction, content parsing, and table extraction into one pass.
pub(super) fn extract_jats_all_in_one(content: &str) -> Result<(JatsMetadataExtracted, String, String, Vec<Table>)> {
    let mut reader = EntityReader::from_str(content);
    let mut budget = SecurityBudget::with_defaults();
    let mut metadata = JatsMetadataExtracted::default();
    let mut body_content = String::new();
    let mut title = String::new();

    let mut in_article_meta = false;
    let mut in_article_title = false;
    let mut in_subtitle = false;
    let mut in_contrib = false;
    let mut in_name = false;
    let mut in_aff = false;
    let mut in_abstract = false;
    let mut in_kwd_group = false;
    let mut in_kwd = false;
    let mut in_history = false;
    let mut in_permissions = false;
    let mut current_author = String::new();
    let mut current_aff = String::new();
    let mut abstract_content = String::new();
    let mut current_contrib_type = String::new();

    let mut in_body = false;
    let mut in_section = false;
    let mut in_para = false;

    let mut in_table = false;
    let mut in_thead = false;
    let mut in_tbody = false;
    let mut in_row = false;
    let mut current_table: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut tables = Vec::new();
    let mut table_index = 0;

    loop {
        budget.step()?;
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                budget.enter()?;
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();

                match tag.as_str() {
                    "article" => {
                        for attr in e.attributes().flatten() {
                            let key = String::from_utf8_lossy(attr.key.as_ref());
                            let val = String::from_utf8_lossy(attr.value.as_ref());
                            budget.check_attr(&key, &val)?;
                            if key == "article-type" {
                                metadata.article_type = Some(val.to_string());
                            }
                        }
                    }
                    "article-meta" => {
                        in_article_meta = true;
                    }
                    "article-title" if in_article_meta => {
                        in_article_title = true;
                    }
                    "subtitle" if in_article_meta => {
                        in_subtitle = true;
                    }
                    "contrib" if in_article_meta => {
                        in_contrib = true;
                        current_author.clear();
                        current_contrib_type.clear();
                        for attr in e.attributes().flatten() {
                            let key = String::from_utf8_lossy(attr.key.as_ref());
                            let val = String::from_utf8_lossy(attr.value.as_ref());
                            budget.check_attr(&key, &val)?;
                            if key == "contrib-type" {
                                current_contrib_type = val.to_string();
                            }
                        }
                    }
                    "name" if in_contrib => {
                        in_name = true;
                    }
                    "aff" if in_article_meta => {
                        in_aff = true;
                        current_aff.clear();
                    }
                    "article-id" if in_article_meta => {
                        let mut id_type = String::new();
                        for attr in e.attributes().flatten() {
                            let key = String::from_utf8_lossy(attr.key.as_ref());
                            let val = String::from_utf8_lossy(attr.value.as_ref());
                            budget.check_attr(&key, &val)?;
                            if key == "pub-id-type" {
                                id_type = val.to_string();
                            }
                        }

                        let id_text = extract_text_content(&mut reader, &mut budget)?;
                        match id_type.as_str() {
                            "doi" => metadata.doi = Some(id_text),
                            "pii" => metadata.pii = Some(id_text),
                            _ => {}
                        }
                        continue;
                    }
                    "volume" if in_article_meta => {
                        let vol_text = extract_text_content(&mut reader, &mut budget)?;
                        metadata.volume = Some(vol_text);
                        continue;
                    }
                    "issue" if in_article_meta => {
                        let issue_text = extract_text_content(&mut reader, &mut budget)?;
                        metadata.issue = Some(issue_text);
                        continue;
                    }
                    "fpage" | "lpage" if in_article_meta => {
                        let page_text = extract_text_content(&mut reader, &mut budget)?;
                        if let Some(pages) = &mut metadata.pages {
                            pages.push('-');
                            pages.push_str(&page_text);
                        } else {
                            metadata.pages = Some(page_text);
                        }
                        continue;
                    }
                    "pub-date" if in_article_meta => {
                        let date_text = extract_text_content(&mut reader, &mut budget)?;
                        if metadata.publication_date.is_none() {
                            metadata.publication_date = Some(date_text);
                        }
                        continue;
                    }
                    "journal-title" if in_article_meta => {
                        let journal_text = extract_text_content(&mut reader, &mut budget)?;
                        if metadata.journal_title.is_none() {
                            metadata.journal_title = Some(journal_text);
                        }
                        continue;
                    }
                    "abstract" if in_article_meta => {
                        in_abstract = true;
                        abstract_content.clear();
                    }
                    "kwd-group" if in_article_meta => {
                        in_kwd_group = true;
                    }
                    "kwd" if in_kwd_group => {
                        in_kwd = true;
                    }
                    "corresp" if in_article_meta => {
                        let corresp_text = extract_text_content(&mut reader, &mut budget)?;
                        metadata.corresponding_author = Some(corresp_text);
                        continue;
                    }
                    "history" if in_article_meta => {
                        in_history = true;
                    }
                    "date" if in_history => {
                        let mut date_type = String::new();
                        for attr in e.attributes().flatten() {
                            let key = String::from_utf8_lossy(attr.key.as_ref());
                            let val = String::from_utf8_lossy(attr.value.as_ref());
                            budget.check_attr(&key, &val)?;
                            if key == "date-type" {
                                date_type = val.to_string();
                            }
                        }
                        let date_text = extract_text_content(&mut reader, &mut budget)?;
                        if !date_text.is_empty() && !date_type.is_empty() {
                            metadata.history_dates.push((date_type, date_text));
                        }
                        continue;
                    }
                    "permissions" if in_article_meta => {
                        in_permissions = true;
                    }
                    "copyright-statement" if in_permissions => {
                        let text = extract_text_content(&mut reader, &mut budget)?;
                        if !text.is_empty() {
                            metadata.copyright_statement = Some(text);
                        }
                        continue;
                    }
                    "license" if in_permissions => {
                        let text = extract_text_content(&mut reader, &mut budget)?;
                        if !text.is_empty() {
                            metadata.license = Some(text);
                        }
                        continue;
                    }
                    "body" => {
                        in_body = true;
                    }
                    "sec" if in_body => {
                        in_section = true;
                    }
                    "title" if (in_section || in_body) && !in_article_title => {
                        let section_title = extract_text_content(&mut reader, &mut budget)?;
                        if !section_title.is_empty() {
                            body_content.push_str("## ");
                            body_content.push_str(&section_title);
                            body_content.push_str("\n\n");
                        }
                        continue;
                    }
                    "p" if in_body || in_section => {
                        in_para = true;
                    }
                    "inline-formula" if in_body => {
                        let latex = extract_formula_latex(&mut reader, &mut budget)?;
                        if !latex.is_empty() {
                            body_content.push('$');
                            body_content.push_str(&latex);
                            body_content.push_str("$ ");
                        }
                        continue;
                    }
                    "table" => {
                        in_table = true;
                        current_table.clear();
                    }
                    "thead" if in_table => {
                        in_thead = true;
                    }
                    "tbody" if in_table => {
                        in_tbody = true;
                    }
                    "tr" if (in_thead || in_tbody) && in_table => {
                        in_row = true;
                        current_row.clear();
                    }
                    "td" | "th" if in_row => {
                        let mut cell_text = String::new();
                        let mut cell_depth = 0;

                        loop {
                            budget.step()?;
                            match reader.read_event() {
                                Ok(Event::Start(_)) => {
                                    budget.enter()?;
                                    cell_depth += 1;
                                }
                                Ok(Event::End(e)) => {
                                    budget.leave();
                                    let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                                    if (tag == "td" || tag == "th") && cell_depth == 0 {
                                        break;
                                    }
                                    if cell_depth > 0 {
                                        cell_depth -= 1;
                                    }
                                }
                                Ok(Event::Text(t)) => {
                                    let decoded = String::from_utf8_lossy(t.as_ref()).to_string();
                                    if !decoded.trim().is_empty() {
                                        budget.check_entity(decoded.trim())?;
                                        budget.account_text(decoded.trim().len())?;
                                        if !cell_text.is_empty() {
                                            cell_text.push(' ');
                                        }
                                        cell_text.push_str(decoded.trim());
                                    }
                                }
                                Ok(Event::Eof) => break,
                                Err(e) => {
                                    return Err(crate::error::XbergError::parsing(format!("XML parsing error: {}", e)));
                                }
                                _ => {}
                            }
                        }

                        budget.add_cells(1)?;
                        current_row.push(cell_text);
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                budget.leave();
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();

                match tag.as_str() {
                    "article-meta" => {
                        in_article_meta = false;
                    }
                    "article-title" if in_article_title => {
                        in_article_title = false;
                    }
                    "subtitle" if in_subtitle => {
                        in_subtitle = false;
                    }
                    "contrib" if in_contrib => {
                        if !current_author.is_empty() {
                            metadata.authors.push(current_author.clone());
                            if !current_contrib_type.is_empty() {
                                metadata
                                    .contributor_roles
                                    .push((current_author.clone(), current_contrib_type.clone()));
                            }
                        }
                        in_contrib = false;
                        current_author.clear();
                        current_contrib_type.clear();
                    }
                    "name" if in_name => {
                        in_name = false;
                    }
                    "aff" if in_aff => {
                        if !current_aff.is_empty() {
                            metadata.affiliations.push(current_aff.clone());
                        }
                        in_aff = false;
                        current_aff.clear();
                    }
                    "abstract" if in_abstract => {
                        in_abstract = false;
                        metadata.abstract_text = Some(abstract_content.trim().to_string());
                    }
                    "history" if in_history => {
                        in_history = false;
                    }
                    "permissions" if in_permissions => {
                        in_permissions = false;
                    }
                    "kwd-group" if in_kwd_group => {
                        in_kwd_group = false;
                    }
                    "kwd" if in_kwd => {
                        in_kwd = false;
                    }
                    "body" => {
                        in_body = false;
                    }
                    "sec" if in_section => {
                        in_section = false;
                    }
                    "p" if in_para => {
                        in_para = false;
                    }
                    "table" if in_table => {
                        if !current_table.is_empty() {
                            let markdown = cells_to_markdown(&current_table);
                            tables.push(Table {
                                cells: current_table.clone(),
                                markdown,
                                page_number: table_index + 1,
                                bounding_box: None,
                                ..Default::default()
                            });
                            table_index += 1;
                            current_table.clear();
                        }
                        in_table = false;
                    }
                    "thead" if in_thead => {
                        in_thead = false;
                    }
                    "tbody" if in_tbody => {
                        in_tbody = false;
                    }
                    "tr" if in_row => {
                        if !current_row.is_empty() {
                            current_table.push(current_row.clone());
                            current_row.clear();
                        }
                        in_row = false;
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(t)) => {
                let decoded = String::from_utf8_lossy(t.as_ref()).to_string();
                let trimmed = decoded.trim();

                if !trimmed.is_empty() {
                    budget.check_entity(trimmed)?;
                    budget.account_text(trimmed.len())?;
                    if in_article_title && metadata.title.is_empty() {
                        metadata.title.push_str(trimmed);
                    } else if in_subtitle && metadata.subtitle.is_none() {
                        metadata.subtitle = Some(trimmed.to_string());
                    } else if in_name {
                        if !current_author.is_empty() {
                            current_author.push(' ');
                        }
                        current_author.push_str(trimmed);
                    } else if in_aff {
                        if !current_aff.is_empty() {
                            current_aff.push(' ');
                        }
                        current_aff.push_str(trimmed);
                    } else if in_abstract {
                        if !abstract_content.is_empty() {
                            abstract_content.push(' ');
                        }
                        abstract_content.push_str(trimmed);
                    } else if in_kwd {
                        metadata.keywords.push(trimmed.to_string());
                    } else if in_para && in_body {
                        body_content.push_str(trimmed);
                        body_content.push_str("\n\n");
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(crate::error::XbergError::parsing(format!("XML parsing error: {}", e)));
            }
            _ => {}
        }
    }

    let mut final_output = body_content;
    if !metadata.title.is_empty() {
        final_output = format!("# {}\n\n{}", metadata.title, final_output);
        title = metadata.title.clone();
    }

    Ok((metadata, final_output.trim().to_string(), title, tables))
}
