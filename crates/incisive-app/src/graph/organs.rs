//! The fixed list of body areas a run can be filed under.

/// The only organ names the picks may use, in display order.
pub const ORGANS: [&str; 26] = [
    "Oesophagus",
    "Stomach",
    "Small bowel",
    "Colon and rectum",
    "Anus",
    "Liver",
    "Gallbladder and bile ducts",
    "Pancreas",
    "Spleen",
    "Abdominal wall",
    "Peritoneum",
    "Endocrine (thyroid, parathyroid, adrenal)",
    "Breast",
    "Lung and chest",
    "Heart and vessels",
    "Brain and spine",
    "Kidney and urinary tract",
    "Reproductive organs",
    "Bone and joint",
    "Skin and soft tissue",
    "Eye",
    "Ear, nose and throat",
    "Mouth and teeth",
    "Blood and immune system",
    "Mental health",
    "Whole body or other",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_is_26_distinct_names() {
        let mut v = ORGANS.to_vec();
        v.sort();
        v.dedup();
        assert_eq!(v.len(), 26);
        assert_eq!(ORGANS[0], "Oesophagus");
        assert_eq!(ORGANS[25], "Whole body or other");
    }
}
