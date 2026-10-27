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
    // --- fix round 1: PIN with place/label context
    ("Bengaluru-560038", &[K::Address]),
    ("Whitefield, Bangalore 560066", &[K::Address]),
    ("Bangalore 560 038", &[K::Address]),
    ("PIN: 560038", &[K::Address]),
    ("PIN code 560038", &[K::Address]),
    ("4th Cross, Indiranagar", &[K::Address]),
    // hospital IDs
    ("UHID no. 4471", &[K::HospitalId]),
    ("uhid- MH 12345", &[K::HospitalId]),
    ("MR no 4471", &[K::HospitalId]),
    ("MR No. 4471", &[K::HospitalId]),
    ("MRD no 4471", &[K::HospitalId]),
    ("Hosp no 4471", &[K::HospitalId]),
    ("Hospital No: 4471", &[K::HospitalId]),
    ("IP 2231/24", &[K::HospitalId]),
    ("Bed 12, IP 4471", &[K::HospitalId]),
    // names
    ("Mr.Ramesh", &[K::PersonName]),
    ("MRS LAKSHMI 52F", &[K::PersonName]),
    ("SHRI GOPAL", &[K::PersonName]),
    ("my patient, Ramesh, 52M", &[K::PersonName]),
    ("Name - Ramesh Kumar", &[K::PersonName]),
    ("s/o Ramesh", &[K::PersonName]),
    ("w/o Suresh Kumar", &[K::PersonName]),
    ("D/O Ramesh", &[K::PersonName]),
    ("Mrs. Hartmann 52F", &[K::PersonName]),
    ("Mr. Lewis, 60M", &[K::PersonName]),
    ("Mr. Miles 62M", &[K::PersonName]),
    // addresses
    ("Flat 302, Prestige Towers", &[K::Address]),
    ("Plot 12, Sector 5", &[K::Address]),
    ("No. 45, MG Road, Bengaluru", &[K::Address]),
    ("45 MG Road", &[K::Address]),
    ("Koramangala 5th Block", &[K::Address]),
    ("Door No 12", &[K::Address]),
    // unicode separators and spacing
    ("98450\u{00a0}12345", &[K::Phone]),
    ("98450\u{2013}12345", &[K::Phone]),
    ("2345\u{00a0}6789\u{00a0}0123", &[K::Aadhaar]),
    ("2345\u{202f}6789\u{2009}0123", &[K::Aadhaar]),
    ("12\u{2013}Mar\u{2013}2024", &[K::CalendarDate]),
    ("12\u{00a0}March\u{00a0}2024", &[K::CalendarDate]),
    ("ramesh\u{ff20}gmail.com", &[K::Email]),
    ("98 450 12345", &[K::Phone]),
    ("2345.6789.0123", &[K::Aadhaar]),
    ("2345  6789  0123", &[K::Aadhaar]),
    ("12-Mar-2024", &[K::CalendarDate]),
    ("12-Mar-24", &[K::CalendarDate]),
    // --- fix round 2
    ("S/O. RAMESH", &[K::PersonName]),
    ("W/o Sri Ramesh", &[K::PersonName]),
    ("c/o Ramesh, 45M", &[K::PersonName]),
    ("c/o Kamal 45 yrs", &[K::PersonName]),
    ("MR RAMESH KUMAR", &[K::PersonName]),
    ("Bangalore 560038 / Karnataka", &[K::Address]),
    ("PAN abcde1234f", &[K::Pan]),
    ("PAN no: abcde1234f", &[K::Pan]),
    ("Patient - Ramesh Kumar", &[K::PersonName]),
    ("S/o Gopal", &[K::PersonName]),
    ("s/o Vishal", &[K::PersonName]),
    ("s/o Kunal, 23M", &[K::PersonName]),
    ("s/o Sonal", &[K::PersonName]),
    ("s/o Kunal", &[K::PersonName]),
    ("w/o Kamal", &[K::PersonName]),
    ("D/o Sonia", &[K::PersonName]),
    ("S/O. GOPAL", &[K::PersonName]),
    ("W/O GOPAL", &[K::PersonName]),
    ("S/O GOPAL KRISHNA", &[K::PersonName]),
    ("s/o Gopal Krishna", &[K::PersonName]),
    ("w/o Gopal Kumar", &[K::PersonName]),
    ("c/o Vishal Kumar", &[K::PersonName]),
    ("c/o Kunal Shah", &[K::PersonName]),
    ("w/o Kunal Shah", &[K::PersonName]),
    ("w/o Sonia Gandhi", &[K::PersonName]),
    ("c/o Kamala, aged 45", &[K::PersonName]),
    ("s/o:Ramesh", &[K::PersonName]),
    ("S/o- Ramesh", &[K::PersonName]),
    ("son of Ramesh", &[K::PersonName]),
    ("wife of Suresh Kumar", &[K::PersonName]),
    ("s/o Dr. Ramesh", &[K::PersonName]),
    ("s/o late Ramaiah", &[K::PersonName]),
    ("h/o Ramesh, 60M", &[K::PersonName]),
    ("s/o Late Sri Ramaiah", &[K::PersonName]),
    ("Name: Kunal Shah", &[K::PersonName]),
    ("CRP 12, mobile 9845012345", &[K::Phone]),
    ("WBC 11000 count, call 9845012345", &[K::Phone]),
    ("attender 9845012345 per son", &[K::Phone]),
    ("ph 9845012345 MG Road", &[K::Phone]),
    ("contact 98450 12345 ml-ward", &[K::Phone]),
    ("Bengaluru 560038 and phone below", &[K::Address]),
    ("resides Whitefield 560066 with wife", &[K::Address]),
    (
        "Address: Whitefield, Bengaluru 560066 for review",
        &[K::Address],
    ),
    ("House No. 45 G Block", &[K::Address]),
    ("Door No 12 G Floor", &[K::Address]),
    ("Flat 302 G Wing, Prestige", &[K::Address]),
    ("IP 4471 M 45y", &[K::HospitalId]),
    ("IP 4471 m", &[K::HospitalId]),
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
    // --- fix round 1: six-digit counts are not PINs
    "a cohort of 150000 patients",
    "sample size 120000 patients",
    "150000 cases",
    "n = 240000",
    "NSQIP 2015-2019 data of 234567 patients",
    "AFP 400000",
    "ferritin 120000",
    "D-dimer 500000",
    "viral load 250000 copies",
    "CA 19-9 > 100000",
    "cost 250000 rupees",
    "Bilirubin 123456 mg",
    // cross is not always a street
    "12 cross-sectional studies",
    "3 cross-over trials",
    "the 2nd cross-match",
    "4th cross-sectional",
    // eponyms in procedure context
    "Mr. Hartmann's procedure",
    "Ivor Lewis operation",
    "Mr Hartmann pouch",
    // words after patient/titles
    "Patient Blood Management",
    "Patient Controlled Analgesia",
    "Ms Excel",
    "Master Class",
    "Miss Rate",
    "patient Murphy sign positive",
    "patient Courvoisier",
    "patient Boerhaave",
    "patient Charlson index 4",
    "patient Caprini 5",
    "patient Kaplan-Meier",
    "patient Gleason 7",
    "patient Fontaine IV",
    "c/o abdominal pain",
    "C/O Abdominal pain for 3 days",
    "w/o jaundice",
    // '#' and main without a road
    "#12 French catheter, main bile duct",
    "port #3, main working port",
    "6th main, leak",
    // --- fix round 2: clinical abbreviations
    "c/o Dyspepsia",
    "c/o Heartburn",
    "c/o Haematemesis",
    "c/o Hematemesis",
    "c/o Malena",
    "c/o Pruritus",
    "c/o Reflux",
    "c/o Regurgitation",
    "c/o Obstipation",
    "c/o Hernia",
    "c/o Haemorrhoids",
    "c/o Painless jaundice",
    "c/o Severe pain RIF",
    "c/o Sudden onset pain",
    "c/o Intermittent pain",
    "c/o Inguinal swelling",
    "c/o Altered bowel habits",
    "c/o Fullness after meals",
    "c/o Passing blood",
    "USG s/o Cholecystitis",
    "CT s/o Perforation",
    "HPE s/o Adenocarcinoma",
    "Imaging s/o Malignancy",
    "USG s/o Grade III fatty liver",
    "USG s/o Appendicitis",
    "w/o Contrast",
    "CT abdomen w/o Contrast",
    "h/o Diabetes",
    // imaging headings
    "MR PELVIS WITH CONTRAST",
    "MR ENTEROGRAPHY",
    "MR CHOLANGIOPANCREATOGRAPHY",
    "MRS SCORE",
    "MR ABDOMEN",
    "MR LIVER",
    // hyphenated patient- compounds
    "Individual Patient-Level Data",
    "Patient-Centred Care",
    "Patient-Specific Instruments",
    // measurements
    "Flat 8 mm polyp in sigmoid",
    "Flat 2 cm lesion",
    "IP 500 mg cisplatin",
    "Mr. Kocher manoeuvre was done",
    "platelets 65000\u{2013}80000",
    "65000-80000/cumm",
    "Approximately 250000 procedures",
    "Overall 234567 admissions",
    "Leucocytes 120000",
    "Medicare 123456 beneficiaries",
    "Japan 150000 gastrectomies",
    // numbered cases
    "Case No. 2, A 60-year-old male",
    "Table No. 3, The results",
    "s/o Mucocele",
    "s/o Abscess",
    "s/o Ascites",
    "s/o Fistula",
    "s/o Volvulus",
    "s/o Leak",
    "s/o Small Bowel Obstruction",
    "CT s/o Bowel Wall Thickening",
    "d/o Hirschsprung",
    "w/o Injury",
    "w/o Drain",
    "Robotic TME done w/o Conversion",
    "C/O PAIN ABDOMEN",
    "S/O ACUTE APPENDICITIS",
    "c/o Mild pain at port site",
    "MRCP s/o Choledocholithiasis",
    "platelets 65000\u{2013}80000",
    "Platelets 85000-120000/cumm",
    "65000-80000/cumm",
    "Overall 120000 robotic cases",
    "IP 200 ml/hr fluids",
    "Flat 30-degree scope",
    "Patient, Seen in OPD",
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

#[test]
fn full_date_span_covers_the_year() {
    for t in ["12-Mar-2024", "12-Mar-24"] {
        let f = scan(t);
        assert_eq!(f.len(), 1, "{t}: {f:?}");
        assert_eq!((f[0].start, f[0].end), (0, t.len()), "{t}");
    }
    let t = "seen \u{2013} 12\u{2013}Mar\u{2013}2024 ok";
    let f = scan(t);
    assert_eq!(&t[f[0].start..f[0].end], "12\u{2013}Mar\u{2013}2024");
}

#[test]
fn month_year_stays_allowed() {
    assert!(scan("in March 2024").is_empty());
}

#[test]
fn six_digit_counts_scan_fast() {
    let text = "123456 mg ".repeat(100_000);
    let t = std::time::Instant::now();
    assert!(scan(&text).is_empty());
    // 300 ms in release (measured ~70 ms); debug builds of regex are ~30x slower, so
    // allow 10 s there: still catches any quadratic behaviour.
    let limit = if cfg!(debug_assertions) { 10_000 } else { 300 };
    assert!(t.elapsed().as_millis() < limit, "took {:?}", t.elapsed());
}

#[test]
fn name_spans_cover_the_whole_name() {
    for (t, want) in [
        ("s/o Late Sri Ramaiah", "s/o Late Sri Ramaiah"),
        ("Name: Kunal Shah", "Name: Kunal Shah"),
        ("seen s/o Gopal Krishna today", "s/o Gopal Krishna"),
        ("Mrs. Lakshmi UHID 4421", "Mrs. Lakshmi"),
    ] {
        let f = scan(t);
        assert_eq!(&t[f[0].start..f[0].end], want, "{t}");
    }
}
