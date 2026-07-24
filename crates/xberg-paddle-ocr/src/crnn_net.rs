use ndarray::Array4;
use ort::session::Session;
use ort::value::Tensor;
use ort::{inputs, session::builder::SessionBuilder};
use std::collections::HashMap;

use crate::{
    base_net::BaseNet,
    constants::{CRNN_MEAN_VALUES, CRNN_NORM_VALUES},
    ocr_error::OcrError,
    ocr_result::TextLine,
    ocr_utils::OcrUtils,
};

const CRNN_DST_HEIGHT: u32 = 48;
const CTC_SPACE_TOKEN: &str = " ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecognitionDictionaryLayout {
    Complete,
    MissingBlank,
    MissingBlankAndSpace,
}

impl RecognitionDictionaryLayout {
    fn resolve(dictionary_len: usize, class_count: usize) -> Result<Self, OcrError> {
        match class_count.checked_sub(dictionary_len) {
            Some(0) => Ok(Self::Complete),
            Some(1) => Ok(Self::MissingBlank),
            Some(2) => Ok(Self::MissingBlankAndSpace),
            _ => Err(OcrError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Recognition dictionary has {dictionary_len} entries but model output has \
                     {class_count} classes; expected an equal class count or exactly one/two \
                     additional CTC classes"
                ),
            ))),
        }
    }

    fn token(self, class_index: usize, keys: &[String]) -> Option<&str> {
        match self {
            Self::Complete => keys.get(class_index).map(String::as_str),
            Self::MissingBlank => keys.get(class_index.checked_sub(1)?).map(String::as_str),
            Self::MissingBlankAndSpace if class_index == keys.len() + 1 => Some(CTC_SPACE_TOKEN),
            Self::MissingBlankAndSpace => keys.get(class_index.checked_sub(1)?).map(String::as_str),
        }
    }
}

#[derive(Debug)]
pub struct CrnnNet {
    session: Option<Session>,
    keys: Vec<String>,
    input_names: Vec<String>,
}

impl BaseNet for CrnnNet {
    fn new() -> Self {
        Self {
            session: None,
            keys: Vec::new(),
            input_names: Vec::new(),
        }
    }

    fn set_input_names(&mut self, input_names: Vec<String>) {
        self.input_names = input_names;
    }

    fn set_session(&mut self, session: Option<Session>) {
        self.session = session;
    }
}

impl CrnnNet {
    pub fn init_model(
        &mut self,
        path: &str,
        num_thread: usize,
        builder_fn: Option<fn(SessionBuilder) -> Result<SessionBuilder, ort::Error>>,
    ) -> Result<(), OcrError> {
        BaseNet::init_model(self, path, num_thread, builder_fn)?;

        self.keys = self.get_keys()?;

        Ok(())
    }

    pub fn init_model_dict_file(
        &mut self,
        path: &str,
        num_thread: usize,
        builder_fn: Option<fn(SessionBuilder) -> Result<SessionBuilder, ort::Error>>,
        dict_file_path: &str,
    ) -> Result<(), OcrError> {
        BaseNet::init_model(self, path, num_thread, builder_fn)?;

        self.read_keys_from_file(dict_file_path)?;

        Ok(())
    }

    pub fn init_model_from_memory(
        &mut self,
        model_bytes: &[u8],
        num_thread: usize,
        builder_fn: Option<fn(SessionBuilder) -> Result<SessionBuilder, ort::Error>>,
    ) -> Result<(), OcrError> {
        BaseNet::init_model_from_memory(self, model_bytes, num_thread, builder_fn)?;

        self.keys = self.get_keys()?;

        Ok(())
    }

    fn get_keys(&mut self) -> Result<Vec<String>, OcrError> {
        let session = self.session.as_ref().ok_or(OcrError::SessionNotInitialized)?;

        let metadata = session.metadata()?;
        let model_charater_list = metadata.custom("character").ok_or_else(|| {
            OcrError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "crnn_net character not found in metadata",
            ))
        })?;

        let keys: Vec<String> = model_charater_list.split('\n').map(|s: &str| s.to_string()).collect();

        Ok(keys)
    }

    fn read_keys_from_file(&mut self, path: &str) -> Result<(), OcrError> {
        let content = std::fs::read_to_string(path)?;

        self.keys = content.lines().map(str::to_string).collect();
        Ok(())
    }

    pub fn get_text_lines(
        &self,
        part_imgs: &[image::RgbImage],
        angle_rollback_records: &HashMap<usize, image::RgbImage>,
        angle_rollback_threshold: f32,
        batch_size: u32,
    ) -> Result<Vec<TextLine>, OcrError> {
        if part_imgs.is_empty() {
            return Ok(Vec::new());
        }

        let mut text_lines = self.get_text_lines_batched(part_imgs, batch_size)?;

        for (index, text_line) in text_lines.iter_mut().enumerate() {
            if (text_line.text_score.is_nan() || text_line.text_score < angle_rollback_threshold)
                && let Some(angle_rollback_record) = angle_rollback_records.get(&index)
            {
                *text_line = self.get_text_line(angle_rollback_record)?;
            }
        }

        Ok(text_lines)
    }

    /// Batch recognition: sort crops by width, group into batches, pad to max width,
    /// run single ONNX inference per batch. Matches PaddleOCR/RapidOCR batching strategy.
    fn get_text_lines_batched(
        &self,
        part_imgs: &[image::RgbImage],
        batch_size: u32,
    ) -> Result<Vec<TextLine>, OcrError> {
        let session = self.session.as_ref().ok_or(OcrError::SessionNotInitialized)?;
        let batch_size = (batch_size as usize).max(1);

        let mut indexed_widths: Vec<(usize, u32)> = part_imgs
            .iter()
            .enumerate()
            .map(|(i, img)| {
                let scale = CRNN_DST_HEIGHT as f32 / img.height().max(1) as f32;
                let dst_width = (img.width() as f32 * scale).ceil() as u32;
                (i, dst_width.max(1))
            })
            .collect();
        indexed_widths.sort_by_key(|&(_, w)| w);

        let mut results: Vec<(usize, TextLine)> = Vec::with_capacity(part_imgs.len());

        for chunk in indexed_widths.chunks(batch_size) {
            if chunk.len() == 1 {
                let (orig_idx, _) = chunk[0];
                let text_line = self.get_text_line(&part_imgs[orig_idx])?;
                results.push((orig_idx, text_line));
                continue;
            }

            let max_width = chunk.iter().map(|&(_, w)| w).max().unwrap_or(1);

            let n = chunk.len();
            let mut batch_data = Array4::<f32>::zeros((n, 3, CRNN_DST_HEIGHT as usize, max_width as usize));

            for (batch_idx, &(orig_idx, dst_width)) in chunk.iter().enumerate() {
                let img = &part_imgs[orig_idx];
                let resized =
                    image::imageops::resize(img, dst_width, CRNN_DST_HEIGHT, image::imageops::FilterType::Triangle);

                let cols = resized.width() as usize;
                let rows = resized.height() as usize;
                let raw = resized.as_raw();
                assert_eq!(raw.len(), rows * cols * 3, "unexpected image buffer size");
                let adjusted = [
                    CRNN_MEAN_VALUES[0] * CRNN_NORM_VALUES[0],
                    CRNN_MEAN_VALUES[1] * CRNN_NORM_VALUES[1],
                    CRNN_MEAN_VALUES[2] * CRNN_NORM_VALUES[2],
                ];
                for r in 0..rows {
                    for c in 0..cols {
                        let base = r * cols * 3 + c * 3;
                        for ch in 0..3 {
                            batch_data[[batch_idx, ch, r, c]] =
                                raw[base + ch] as f32 * CRNN_NORM_VALUES[ch] - adjusted[ch];
                        }
                    }
                }
            }

            let input_tensor = Tensor::from_array(batch_data)?;

            #[allow(unsafe_code)]
            let outputs = unsafe {
                let session_ptr = session as *const Session as *mut Session;
                (*session_ptr).run(inputs![self.input_names[0].as_str() => input_tensor])?
            };

            let (_, output_value) = outputs.iter().next().ok_or_else(|| {
                OcrError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "No output tensors found in batched CRNN session output",
                ))
            })?;

            let (shape, flat_data) = output_value.try_extract_tensor::<f32>()?;
            let batch_dim = *shape.first().unwrap_or(&1) as usize;
            let timesteps = *shape.get(1).unwrap_or(&0) as usize;
            let num_classes = *shape.get(2).unwrap_or(&0) as usize;

            for (batch_idx, item) in chunk.iter().enumerate().take(batch_dim.min(n)) {
                let offset = batch_idx * timesteps * num_classes;
                let slice = &flat_data[offset..offset + timesteps * num_classes];
                let text_line = Self::score_to_text_line(slice, timesteps, num_classes, &self.keys)?;
                results.push((item.0, text_line));
            }
        }

        results.sort_by_key(|&(idx, _)| idx);
        Ok(results.into_iter().map(|(_, tl)| tl).collect())
    }

    fn get_text_line(&self, img_src: &image::RgbImage) -> Result<TextLine, OcrError> {
        let Some(session) = &self.session else {
            return Err(OcrError::SessionNotInitialized);
        };

        let scale = CRNN_DST_HEIGHT as f32 / img_src.height() as f32;
        let dst_width = (img_src.width() as f32 * scale).ceil() as u32;

        let src_resize = image::imageops::resize(
            img_src,
            dst_width,
            CRNN_DST_HEIGHT,
            image::imageops::FilterType::Triangle,
        );

        let input_tensors = OcrUtils::substract_mean_normalize(&src_resize, &CRNN_MEAN_VALUES, &CRNN_NORM_VALUES);

        let input_tensors = Tensor::from_array(input_tensors)?;

        #[allow(unsafe_code)]
        let outputs = unsafe {
            let session_ptr = session as *const Session as *mut Session;
            (*session_ptr).run(inputs![self.input_names[0].as_str() => input_tensors])?
        };

        let (_, red_data) = outputs.iter().next().ok_or_else(|| {
            OcrError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "No output tensors found in CRNN session output",
            ))
        })?;

        let (shape, src_data) = red_data.try_extract_tensor::<f32>()?;
        let dimensions = shape;
        let height = *dimensions.get(1).ok_or_else(|| {
            OcrError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "CRNN output tensor missing height dimension (index 1)",
            ))
        })? as usize;
        let width = *dimensions.get(2).ok_or_else(|| {
            OcrError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "CRNN output tensor missing width dimension (index 2)",
            ))
        })? as usize;
        let src_data: Vec<f32> = src_data.to_vec();

        Self::score_to_text_line(&src_data, height, width, &self.keys)
    }

    fn score_to_text_line(
        output_data: &[f32],
        height: usize,
        width: usize,
        keys: &[String],
    ) -> Result<TextLine, OcrError> {
        let dictionary_layout = RecognitionDictionaryLayout::resolve(keys.len(), width)?;
        let mut text_line = TextLine::default();
        let mut last_index = 0;

        let mut text_score_sum = 0.0;
        let mut text_score_count = 0;
        for i in 0..height {
            let start = i * width;
            let stop = (i + 1) * width;
            let slice = &output_data[start..stop.min(output_data.len())];

            let (max_index, max_value) =
                slice
                    .iter()
                    .enumerate()
                    .fold((0, f32::MIN), |(max_idx, max_val), (idx, &val)| {
                        if val > max_val { (idx, val) } else { (max_idx, max_val) }
                    });

            if max_index > 0
                && !(i > 0 && max_index == last_index)
                && let Some(token) = dictionary_layout.token(max_index, keys)
            {
                text_line.text.push_str(token);
                text_score_sum += max_value;
                text_score_count += 1;
            }
            last_index = max_index;
        }

        text_line.text_score = if text_score_count > 0 {
            text_score_sum / text_score_count as f32
        } else {
            0.0
        };
        Ok(text_line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output_for_classes(class_count: usize, classes: &[usize]) -> Vec<f32> {
        let mut output = vec![0.0; class_count * classes.len()];
        for (timestep, class_index) in classes.iter().copied().enumerate() {
            output[timestep * class_count + class_index] = 1.0;
        }
        output
    }

    #[test]
    fn test_score_to_text_line_skips_blank_index() {
        let keys = vec!["#".to_string(), "a".to_string(), "b".to_string()];
        let output = vec![1.0, 0.0, 0.0, 0.0, 0.9, 0.1, 0.0, 0.1, 0.8];
        let result = CrnnNet::score_to_text_line(&output, 3, 3, &keys).unwrap();
        assert_eq!(result.text, "ab");
    }

    #[test]
    fn test_score_to_text_line_deduplicates_consecutive() {
        let keys = vec!["#".to_string(), "h".to_string(), "i".to_string()];
        let output = vec![0.0, 0.9, 0.0, 0.0, 0.8, 0.0, 0.0, 0.0, 0.9, 0.0, 0.0, 0.8];
        let result = CrnnNet::score_to_text_line(&output, 4, 3, &keys).unwrap();
        assert_eq!(result.text, "hi");
    }

    #[test]
    fn test_score_to_text_line_preserves_complete_dictionary() {
        let keys = vec!["#".to_string(), "a".to_string(), " ".to_string()];
        let output = output_for_classes(keys.len(), &[0, 1, 2]);
        let result = CrnnNet::score_to_text_line(&output, 3, keys.len(), &keys).unwrap();
        assert_eq!(result.text, "a ");
    }

    #[test]
    fn test_score_to_text_line_prepends_missing_blank() {
        let keys = vec!["a".to_string(), "b".to_string()];
        let class_count = keys.len() + 1;
        let output = output_for_classes(class_count, &[0, 1, 2]);
        let result = CrnnNet::score_to_text_line(&output, 3, class_count, &keys).unwrap();
        assert_eq!(result.text, "ab");
    }

    #[test]
    fn test_score_to_text_line_adds_missing_blank_and_space() {
        let keys = vec!["a".to_string(), "b".to_string()];
        let class_count = keys.len() + 2;
        let output = output_for_classes(class_count, &[0, 1, 2, 3]);
        let result = CrnnNet::score_to_text_line(&output, 4, class_count, &keys).unwrap();
        assert_eq!(result.text, "ab ");
    }

    #[test]
    fn test_score_to_text_line_rejects_dictionary_class_mismatch() {
        let keys = vec!["a".to_string()];
        let class_count = keys.len() + 3;
        let output = output_for_classes(class_count, &[1]);
        let error = CrnnNet::score_to_text_line(&output, 1, class_count, &keys).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("dictionary has 1 entries"));
        assert!(message.contains("model output has 4 classes"));
    }

    #[test]
    fn test_read_keys_from_file_preserves_crlf_and_internal_blank() {
        let dir = std::env::temp_dir().join(format!("xberg_test_crnn_dictionary_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dict_path = dir.join("test_dict.txt");
        std::fs::write(&dict_path, "a\r\n\r\nb\r\n").unwrap();

        let mut net = CrnnNet::new();
        net.read_keys_from_file(dict_path.to_str().unwrap()).unwrap();

        assert_eq!(net.keys, ["a", "", "b"]);

        std::fs::remove_dir_all(&dir).ok();
    }
}
