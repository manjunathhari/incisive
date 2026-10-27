use incisive_app::deid::{Finding, FindingKind as K, describe, scan};

/// Distinct kinds found in `text`, in enum order.
fn kinds(text: &str) -> Vec<K> {
    let mut v: Vec<K> = scan(text).into_iter().map(|f| f.kind).collect();
    v.sort();
    v.dedup();
    v
}

/// Text that must be blocked, with the exact set of kinds expected.
const BLOCK: &[(&str, &[K])] = &[
    // Aadhaar
    ("Aadhaar 2345 6789 0123", &[K::Aadhaar]),
    ("Aadhaar 2345-6789-0123", &[K::Aadhaar]),
    ("uid 234567890123 on file", &[K::Aadhaar]),
    // PAN
    ("PAN ABCDE1234F", &[K::Pan]),
    ("pan no. BNZPM2501F given", &[K::Pan]),
    // Phone
    ("ph +91 98450 12345", &[K::Phone]),
    ("mobile 9845012345", &[K::Phone]),
    ("call 080-2502 4444", &[K::Phone]),
    ("tel (080) 2502 4444", &[K::Phone]),
    ("+919845012345", &[K::Phone]),
    ("whatsapp 09845012345", &[K::Phone]),
    ("attender 98450-12345", &[K::Phone]),
    // Email
    ("email ramesh.k@gmail.com", &[K::Email]),
    ("write to dr_x+cases@hospital.co.in please", &[K::Email]),
    // Calendar dates
    ("DOB 12/03/1971", &[K::CalendarDate]),
    ("DOB 5/6/1990", &[K::CalendarDate]),
    ("seen on 2024-03-12", &[K::CalendarDate]),
    ("operated 12-03-2024", &[K::CalendarDate]),
    ("admitted 05.06.19", &[K::CalendarDate]),
    ("on 12 March 2024 she", &[K::CalendarDate]),
    ("March 12, 2024 discharge", &[K::CalendarDate]),
    ("scheduled 12th Mar", &[K::CalendarDate]),
    ("seen 1st Nov 2023", &[K::CalendarDate]),
    ("born 3rd May", &[K::CalendarDate]),
    // Hospital IDs
    ("UHID: MH-2231", &[K::HospitalId]),
    ("IP no 44718", &[K::HospitalId]),
    ("MRN 0012345", &[K::HospitalId]),
    ("OP no. B1234", &[K::HospitalId]),
    ("Patient ID: 99823", &[K::HospitalId]),
    ("Reg no 55/2231", &[K::HospitalId]),
    ("IPD 7788", &[K::HospitalId]),
    ("uhid #MH8812", &[K::HospitalId]),
    // Person names
    ("Mrs. Lakshmi, 52F", &[K::PersonName]),
    ("patient Ramesh had", &[K::PersonName]),
    ("Mr Suresh Kumar underwent", &[K::PersonName]),
    ("Shri Gopal presented", &[K::PersonName]),
    ("Smt. Devi with gallstones", &[K::PersonName]),
    ("Baby of Anitha, 3 kg", &[K::PersonName]),
    ("Kumari Priya, 8 y/o", &[K::PersonName]),
    ("Master Arjun, 6 y/o", &[K::PersonName]),
    ("Ms. Rao, post-op day 3", &[K::PersonName]),
    ("pt called Meena", &[K::PersonName]),
    ("named Prakash", &[K::PersonName]),
    ("name: Rajesh", &[K::PersonName]),
    ("Patient Name: Ramesh Kumar", &[K::PersonName]),
    ("the patient Kavita was", &[K::PersonName]),
    // Addresses
    ("#12, 4th Cross, Indiranagar 560038", &[K::Address]),
    ("House no 14, Jayanagar", &[K::Address]),
    ("Flat no. 302 Prestige Towers", &[K::Address]),
    ("lives at 4th Main Road", &[K::Address]),
    ("HSR Layout 2nd sector", &[K::Address]),
    ("pincode 600001", &[K::Address]),
    ("Pin 560038", &[K::Address]),
    ("door no 7/2, Vijayanagar", &[K::Address]),
    // Several at once
    (
        "Mrs. Lakshmi UHID 4421 ph 9845012345",
        &[K::Phone, K::HospitalId, K::PersonName],
    ),
    (
        "Aadhaar 2345 6789 0123 and PAN ABCDE1234F",
        &[K::Aadhaar, K::Pan],
    ),
];

/// Ordinary clinical text that must pass untouched.
const PASS: &[&str] = &[
    // The brief's named cases
    "52 y/o M, T3N1M0 rectal ca, POD 3, Hb 9.8, CEA 12.4",
    "Whipple vs PPPD in 2019 trial",
    "Clavien-Dindo IIIa leak on day 5",
    "ASA 3, BMI 31",
    "120 mg bd for 6 weeks",
    // Ages, sex, relative time
    "52-year-old male",
    "52F with biliary colic",
    "6 weeks post-op, POD 3, day 5",
    "ECOG 1, ASA 2, POD 5",
    "Grade 2 complication at 6/12",
    // Abbreviations are not names
    "Pt underwent lap chole",
    "pt with T3 tumour",
    "Patient had ileus",
    "Pt Hb 9.8",
    "PT INR 1.2 Normal",
    "Patient Safety in robotic surgery",
    "patient-reported outcome measures",
    "patient selection for Whipple",
    "Master the learning curve",
    "Miss rate of polyps",
    // Eponyms after titles and context words
    "Mr. Hartmann's procedure for perforated diverticulitis",
    "named Whipple procedure",
    "called Hartmann",
    "Child-Pugh B, Dukes C, Billroth II",
    "Heller myotomy vs Nissen",
    "Mr Dukes C staging",
    // Lab values and counts vs phones / PINs
    "platelets 150000",
    "WBC 11200",
    "Hb 9.8",
    "platelets 150000/µL",
    "TLC 450000 cells",
    "count 120000",
    "Hb 9.8, platelets 150000 per mm3",
    "WBC 11200 and platelets 250000",
    "CA 19-9 1200 U/mL",
    "CEA 12.4 ng/mL",
    "accession 9845012345678",
    "ID 12345678901234",
    "dose 1500000 units",
    // Dates that are not calendar dates
    "in 2019",
    "in 2019 trial",
    "in March 2024",
    "1/2",
    "3/12 follow-up",
    "BP 120/80",
    "3 may be required",
    "survival 12.3 months",
    // Address-like clinical text
    "trocar layout 5 mm",
    "nerve block 3 ml",
    "2 stage procedure",
    "pooling #3 sample",
    // Labels without an ID
    "IPD analysis of 8 trials",
    "MRN is recorded in the notes",
    "T3N1M0 ypT2N0, R0 resection",
];

#[test]
fn must_block_table() {
    assert!(BLOCK.len() >= 25);
    let mut bad = Vec::new();
    for (text, want) in BLOCK {
        let got = kinds(text);
        if got != *want {
            bad.push(format!("{text:?}: want {want:?}, got {got:?}"));
        }
    }
    assert!(bad.is_empty(), "mis-classified blocks:\n{}", bad.join("\n"));
}

#[test]
fn must_pass_table() {
    assert!(PASS.len() >= 25);
    let mut bad = Vec::new();
    for text in PASS {
        let got = scan(text);
        if !got.is_empty() {
            bad.push(format!("{text:?}: unexpectedly flagged {got:?}"));
        }
    }
    assert!(bad.is_empty(), "false positives:\n{}", bad.join("\n"));
}

#[test]
fn table_is_big_enough() {
    assert!(BLOCK.len() + PASS.len() >= 60);
}

#[test]
fn offsets_point_at_the_match() {
    let t = "ph 9845012345 now";
    let f = scan(t);
    assert_eq!(f.len(), 1);
    assert_eq!(&t[f[0].start..f[0].end], "9845012345");

    // Non-ASCII text before the match: offsets are bytes, not chars.
    let t = "रोगी Mrs. Lakshmi";
    let f = scan(t);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].kind, K::PersonName);
    assert_eq!(&t[f[0].start..f[0].end], "Mrs. Lakshmi");

    let t = "रोगी का फोन 080-2502 4444 है, ईमेल a@b.in";
    for f in scan(t) {
        assert!(t.is_char_boundary(f.start) && t.is_char_boundary(f.end));
    }
    let f = scan(t);
    assert_eq!(&t[f[0].start..f[0].end], "080-2502 4444");
    assert_eq!(&t[f[1].start..f[1].end], "a@b.in");
}

#[test]
fn findings_are_sorted_by_start() {
    let t = "email a@b.in then PAN ABCDE1234F then ph 9845012345";
    let f = scan(t);
    assert_eq!(f.len(), 3);
    assert!(f.windows(2).all(|w| w[0].start <= w[1].start));
}

#[test]
fn overlaps_are_merged() {
    // HospitalId and Phone overlap on the same digits: one finding, union span.
    let t = "UHID: 9845012345";
    let f = scan(t);
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(&t[f[0].start..f[0].end], t);

    // No two findings may ever overlap.
    let t = "#12, 4th Cross, Indiranagar 560038, Mrs. Lakshmi UHID 4421 ph 9845012345";
    let f = scan(t);
    assert!(f.windows(2).all(|w: &[Finding]| w[0].end <= w[1].start));
}

#[test]
fn repeated_identifiers_are_all_found() {
    let t = "9845012345 and 9845012346";
    assert_eq!(scan(t).len(), 2);
}

#[test]
fn clean_and_empty_text() {
    assert!(scan("").is_empty());
    assert!(scan("Laparoscopic cholecystectomy for symptomatic cholelithiasis").is_empty());
}

#[test]
fn kinds_serialise_snake_case() {
    assert_eq!(
        serde_json::to_string(&K::CalendarDate).unwrap(),
        "\"calendar_date\""
    );
    assert_eq!(
        serde_json::to_string(&K::HospitalId).unwrap(),
        "\"hospital_id\""
    );
    assert_eq!(serde_json::to_string(&K::Aadhaar).unwrap(), "\"aadhaar\"");
}

#[test]
fn every_kind_has_a_label() {
    for k in [
        K::Aadhaar,
        K::Pan,
        K::Phone,
        K::Email,
        K::CalendarDate,
        K::HospitalId,
        K::PersonName,
        K::Address,
    ] {
        assert!(!describe(k).is_empty());
    }
    assert_eq!(describe(K::Phone), "phone number");
}
