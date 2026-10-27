use incisive_app::deid::{Finding, FindingKind as K, describe, hint, scan};

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
    // --- fix round 4
    ("from Whitefield 560066 presents with pain", &[K::Address]),
    (
        "Address: Whitefield, Bengaluru 560066 presents for review",
        &[K::Address],
    ),
    ("Kadugodi, Bengaluru 560067 visits OPD", &[K::Address]),
    ("Bengaluru 560038 attends clinic", &[K::Address]),
    ("MRS SAROJAMMA, 67 F", &[K::PersonName]),
    ("call on 98450 12 345", &[K::Phone]),
    ("s/o Om", &[K::PersonName]),
    // --- fix round 5
    ("S/O MUNIYAPPA OCCUPATION: AGRICULTURIST", &[K::PersonName]),
    (
        "S/O NAGESH GOWDA DIAGNOSIS: RT INGUINAL HERNIA",
        &[K::PersonName],
    ),
    ("D/O SHIVALINGAPPA DIAGNOSIS CA BREAST", &[K::PersonName]),
    ("s/o Venkatappa Station Road, Hoskote", &[K::PersonName]),
    (
        "s/o Thimmaiah Admission Diagnosis Acute Appendicitis",
        &[K::PersonName],
    ),
    ("Son of Muniswamy Diagnosis: SAIO", &[K::PersonName]),
    (
        "Wife of Chandrashekar Occupation Housewife",
        &[K::PersonName],
    ),
    ("s/o Ramappa Station Road", &[K::PersonName]),
    ("c/o Puttaswamy, 56 yrs", &[K::PersonName]),
    ("c/o Puttaswamy, 56 Years, Male", &[K::PersonName]),
    ("w/o Kariyappa, 48 yrs", &[K::PersonName]),
    ("Patient Nancy admitted", &[K::PersonName]),
    ("Pt Soma, 45F", &[K::PersonName]),
    ("Patient Roma Devi", &[K::PersonName]),
    ("Name: Nancy Thomas", &[K::PersonName]),
    // --- fix round 6: label words stop the s/o walk
    (
        "s/o Thimmaiah Diagnosis Acute Appendicitis",
        &[K::PersonName],
    ),
    ("S/O MUNIYAPPA DIAGNOSIS INGUINAL HERNIA", &[K::PersonName]),
    ("s/o Muniyappa Diagnosis Hernia", &[K::PersonName]),
    (
        "S/O MUNIYAPPA IMPRESSION ACUTE APPENDICITIS",
        &[K::PersonName],
    ),
    (
        "Wife of Chandrashekar Diagnosis Carcinoma",
        &[K::PersonName],
    ),
    (
        "D/O Venkatappa Diagnosis Carcinoma Breast",
        &[K::PersonName],
    ),
    ("S/O MUNIYAPPA WITH INGUINAL HERNIA", &[K::PersonName]),
    // years of age
    ("c/o Puttaswamy, 56 years of age", &[K::PersonName]),
    ("W/O MUNIRAJU, 54 years of age", &[K::PersonName]),
    (
        "c/o Puttaswamy , 56 years, history of hernia",
        &[K::PersonName],
    ),
    // case-insensitive name label
    ("NAME: RAMAKKA", &[K::PersonName]),
    ("Name: RAMAKKA", &[K::PersonName]),
    ("NAME: Ramakka", &[K::PersonName]),
    ("NAME : RAMAKKA", &[K::PersonName]),
    ("PATIENT NAME: RAMAKKA", &[K::PersonName]),
    ("NAME: RAMESH", &[K::PersonName]),
    ("NAME: RAMAKKA AGE/SEX: 45/F", &[K::PersonName]),
    ("Pt. Name: RAMAKKA", &[K::PersonName]),
    ("Pt Name - Ramakka", &[K::PersonName]),
    ("Patient Name : RAMAKKA DEVI", &[K::PersonName]),
    // Hidden and look-alike characters (R-F1: rejected, never normalised)
    ("ph 98450\u{200B}12345", &[K::UnusualCharacters]),
    ("ph 9845\u{AD}012345", &[K::UnusualCharacters]),
    (
        "ph \u{FF19}\u{FF18}\u{FF14}\u{FF15}\u{FF10}\u{FF11}\u{FF12}\u{FF13}\u{FF14}\u{FF15}",
        &[K::UnusualCharacters],
    ),
    (
        "Aadhaar 2345\u{200B}6789\u{200B}0123",
        &[K::UnusualCharacters],
    ),
    ("Mrs\u{200B}. Lakshmi", &[K::UnusualCharacters]),
    ("UHID\u{200B}: MH-2231", &[K::UnusualCharacters]),
    ("DOB 12\u{2215}03\u{2215}1971", &[K::UnusualCharacters]),
    // More of the same family
    ("ph 98450\u{2060}12345", &[K::UnusualCharacters]),
    ("ph 98450\u{FEFF}12345", &[K::UnusualCharacters]),
    ("ph \u{202E}5432105489", &[K::UnusualCharacters]),
    ("ph \u{0969}\u{0968}\u{0967}", &[K::UnusualCharacters]),
    ("ph \u{0CEF}\u{0CEE}\u{0CEA}", &[K::UnusualCharacters]),
    ("DOB 12\u{2044}03\u{2044}1971", &[K::UnusualCharacters]),
    ("DOB 12\u{29F8}03\u{29F8}1971", &[K::UnusualCharacters]),
    ("at 10\u{2236}30", &[K::UnusualCharacters]),
    ("at 10\u{FE55}30", &[K::UnusualCharacters]),
    ("at 10\u{FE13}30", &[K::UnusualCharacters]),
    ("ph 98450\u{2012}12345", &[K::UnusualCharacters]),
    ("DOB 12\u{2010}03\u{2010}1971", &[K::UnusualCharacters]),
    ("DOB 12\u{2011}03\u{2011}1971", &[K::UnusualCharacters]),
    ("ph 98450\u{2015}12345", &[K::UnusualCharacters]),
    ("ph 98450\u{2212}12345", &[K::UnusualCharacters]),
    ("DOB 12\u{2027}03\u{2027}1971", &[K::UnusualCharacters]),
    ("ph 98450\u{FF0D}12345", &[K::UnusualCharacters]),
    ("ph 98450\u{2079}2345", &[K::UnusualCharacters]),
    (
        "ph \u{2079}\u{2078}\u{2074}\u{2075}",
        &[K::UnusualCharacters],
    ),
    // Ordinary en/em dashes between digits are handled by the phone and date rules.
    ("ph 98450\u{2013}12345", &[K::Phone]),
    ("DOB 12\u{2013}03\u{2013}1971", &[K::CalendarDate]),
    // --- R-F3 allowlist: leaks found by the re-review
    // invisible characters that are not Cf. Where a kind other than
    // UnusualCharacters is expected, the flagged character sits inside an
    // identifier the rules also find (marks: via the mark-free rescan; NBSP and
    // full-width @: accepted separators) and the merged finding takes its kind.
    ("ph 98450\u{34F}12345", &[K::Phone]),
    ("ph 98450\u{FE0F}12345", &[K::Phone]),
    ("ph 98450\u{115F}12345", &[K::UnusualCharacters]),
    ("ph 98450\u{3164}12345", &[K::UnusualCharacters]),
    ("ph 98450\u{2800}12345", &[K::UnusualCharacters]),
    ("ph 98450\u{1}12345", &[K::UnusualCharacters]),
    ("ph 98450\u{7F}12345", &[K::UnusualCharacters]),
    ("ph 98450\u{85}12345", &[K::UnusualCharacters]),
    ("ph 98450\u{332}12345", &[K::Phone]),
    ("ph 98450\u{20E3}12345", &[K::Phone]),
    // a mark on a letter is allowed, so the identifier rules also scan the
    // text without marks: the name is found
    ("Mrs\u{301}. Lakshmi", &[K::PersonName]),
    ("UHID\u{301}: MH-2231", &[K::HospitalId]),
    ("Mrs.\u{301} Lakshmi", &[K::PersonName]),
    // look-alike colons
    ("UHID\u{A789} MH-2231", &[K::UnusualCharacters]),
    ("UHID\u{2D0} MH-2231", &[K::UnusualCharacters]),
    ("UHID\u{589} MH-2231", &[K::UnusualCharacters]),
    ("UHID\u{5C3} MH-2231", &[K::UnusualCharacters]),
    ("UHID\u{FE30} MH-2231", &[K::UnusualCharacters]),
    ("UHID\u{703} MH-2231", &[K::UnusualCharacters]),
    ("UHID\u{2E35} MH-2231", &[K::UnusualCharacters]),
    // non-ASCII dashes next to a digit
    ("UHID MH\u{2013}2231", &[K::UnusualCharacters]),
    ("UHID MH\u{2011}2231", &[K::UnusualCharacters]),
    ("UHID MH\u{2212}2231", &[K::UnusualCharacters]),
    ("UHID MH\u{2010}2231", &[K::UnusualCharacters]),
    ("UHID MH\u{2014}2231", &[K::UnusualCharacters]),
    // separators
    ("DOB 12\u{B7}03\u{B7}1971", &[K::UnusualCharacters]),
    ("DOB 12\u{2024}03\u{2024}1971", &[K::UnusualCharacters]),
    ("DOB 12\u{3002}03\u{3002}1971", &[K::UnusualCharacters]),
    ("DOB 12\u{2571}03\u{2571}1971", &[K::UnusualCharacters]),
    ("DOB 12\u{3033}03\u{3033}1971", &[K::UnusualCharacters]),
    ("DOB 12\u{338}03\u{338}1971", &[K::UnusualCharacters]),
    ("ph 98450\u{FE63}12345", &[K::UnusualCharacters]),
    ("ph 98450\u{2E3A}12345", &[K::UnusualCharacters]),
    ("ph 98450\u{2E3B}12345", &[K::UnusualCharacters]),
    ("ph 98450\u{FE58}12345", &[K::UnusualCharacters]),
    ("ph 98450\u{2043}12345", &[K::UnusualCharacters]),
    // look-alike spaces between digits
    ("ph 98450\u{2007}12345", &[K::UnusualCharacters]),
    ("ph 98450\u{2002}12345", &[K::UnusualCharacters]),
    ("ph 98450\u{3000}12345", &[K::UnusualCharacters]),
    ("ph 98450\u{205F}12345", &[K::UnusualCharacters]),
    ("ph 98450\u{1680}12345", &[K::UnusualCharacters]),
    ("ph 98450\u{A0}12345", &[K::Phone]),
    ("ph 98450\u{202F}12345", &[K::Phone]),
    // @ variants and mixed-script look-alikes
    ("ramesh\u{FE6B}gmail.com", &[K::UnusualCharacters]),
    ("ramesh\u{FF20}gmail.com", &[K::Email]),
    ("\u{41C}rs. Lakshmi", &[K::UnusualCharacters]),
    ("PAN \u{1D400}\u{1D401}CDE1234F", &[K::UnusualCharacters]),
    // other scripts, Roman numeral characters, emoji (accepted over-blocks)
    (
        "\u{0930}\u{094B}\u{0917}\u{0940} 52",
        &[K::UnusualCharacters],
    ),
    ("Clavien-Dindo \u{2162}a", &[K::UnusualCharacters]),
    ("leak? \u{1F914}", &[K::UnusualCharacters]),
    ("10\u{2153} cm", &[K::UnusualCharacters]),
    // ranges with an en dash between digits: fail-closed over-block (retype with "-")
    ("platelets 65000\u{2013}80000", &[K::UnusualCharacters]),
    ("40\u{2013}60 mg", &[K::UnusualCharacters]),
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
    "platelets 65000-80000",
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
    "platelets 65000-80000",
    "Platelets 85000-120000/cumm",
    "65000-80000/cumm",
    "Overall 120000 robotic cases",
    "IP 200 ml/hr fluids",
    "Flat 30-degree scope",
    "Patient, Seen in OPD",
    // --- fix round 4
    "c/o Lt Inguinal Swelling",
    "c/o Lt Sided Abdominal Pain",
    "s/o Lt Inguinal Hernia",
    "Healed w/o Late Complications",
    "CT s/o Intestinal Obstruction",
    "S/O INTESTINAL OBSTRUCTION",
    "s/o Achalasia (Cardia)",
    "s/o Duodenal Ulcer Perforation",
    "s/o Intussusception",
    "s/o Rectal Growth",
    "s/o Umbilical Hernia",
    "s/o Perianal Fistula",
    "s/o Peritoneal Carcinomatosis",
    "Findings s/o Sigmoid Volvulus",
    "s/o Hepatic Abscess",
    "s/o Pancreatic Pseudocyst",
    "s/o Gastric Outlet Obstruction",
    "s/o Appendicular Lump",
    "s/o Mesenteric Ischemia",
    "s/o Koch's Abdomen",
    "s/o Free Fluid",
    "S/O SUBACUTE INTESTINAL OBSTRUCTION",
    "s/o Gas Under Diaphragm",
    "s/o Fatty Liver",
    "s/o Calculous Cholecystitis",
    "h/o Previous Surgery",
    "h/o Similar Complaints",
    "c/o Heaviness Over Left Groin",
    "Consent from Son of Patient",
    "Wife of Patient informed",
    "h/o Smoking 20 years",
    "h/o Alcohol Intake 15 years",
    "h/o Surgery 10 yrs ago",
    "h/o Asthma 20 years",
    "h/o Tobacco Chewing 30 years",
    "h/o Trauma 12 years back",
    "ABG pH 7.31, Plt 65000-80000",
    "Flat 8 MM polyp",
    "Flat 2 CM lesion",
    "IP 500 MG CISPLATIN",
    "IP 1 g ceftriaxone",
    "HBV DNA 7800000000 IU/mL",
    // --- fix round 5
    "s/o Sebaceous Cyst",
    "s/o Dermoid Cyst",
    "s/o Intestinal Malrotation",
    "s/o Arteriovenous Malformation",
    "s/o Paget Disease",
    "S/O CA RECTUM",
    "Impression: s/o Acute Appendicitis",
    "s/o Carcinoid Tumour",
    "s/o Ovarian Torsion",
    "c/o Giddiness, 2 years",
    "c/o Pain Abdomen, 10 years",
    "c/o Swelling, 10 years duration",
    "c/o Dyspepsia, 12 years",
    "c/o Heartburn, 15 years back",
    "h/o Epilepsy, 12 yrs",
    "CT s/o Annular Pancreas",
    // --- fix round 6
    "s/o Sigmoid Diverticulitis with pericolic abscess",
    "USG s/o Acute Calculous Cholecystitis with GB wall edema",
    "s/o Desmoid Tumour of anterior abdominal wall",
    "c/o Dyspepsia, 12 years of history",
    "c/o Pain, 2 months of duration",
    "Name of the procedure: Whipple",
    "NAME OF SURGERY: LAP CHOLECYSTECTOMY",
    "Brand name: Prolene mesh",
    "Drug name: Tranexamic acid",
    "Trial name: ROLARR",
    "Procedure name: Whipple",
    "PROCEDURE NAME: LAP CHOLECYSTECTOMY",
    "Generic name: Paracetamol",
    "Trade Name: Ultrapro",
    "Device name: Stapler",
    "Study Name: ROLARR",
    "Name: Unknown",
    "PATIENT NAME: WITHHELD",
    "w/o Recurrence, 10 years later",
    "c/o Backache, 10 yrs",
    "Biopsy s/o Signet Ring Cell Carcinoma",
    // --- final fix wave: ordinary Unicode in clinical text
    "Crohn\u{2019}s disease",
    "5 \u{B5}g fentanyl",
    "37.5 \u{B0}C fever",
    "\u{2265} 2 cm margin",
    "\u{2264} 5 mm",
    "\u{3B2}-blocker",
    "caf\u{E9}-au-lait spots",
    "BMI 32 kg/m\u{B2}",
    "BSA 1.8 m\u{B2}, 4 cm\u{B3}",
    "platelets 150 \u{D7}10\u{2079}/L",
    "RBC 4.5 \u{D7}10\u{B9}\u{B2}/L",
    "1\u{BD} hours",
    "\u{BD} tablet",
    "mean 12 \u{B1} 3",
    "HbA1c 7.2 %, SpO\u{2082} 94%",
    "ICG in robotic low anterior resection \u{2014} evidence for leak?",
    "dose 5-10 mg",
    "temperature -2 \u{B0}C",
    "\u{3B2}\u{2010}blocker",
    "mild \u{2010} moderate",
    "pain - worse",
    "\u{201C}leak\u{201D} \u{2026} \u{2018}rates\u{2019}",
    "\u{2022} item one",
    "\u{2192} next step",
    "Ki-67 index",
    // --- R-F3 allowlist
    "caf\u{E9}-au-lait spots",
    "cafe\u{301}-au-lait spots",
    "m\u{B2}",
    "\u{D7}10\u{2079}/L",
    "SpO\u{2082}",
    "Ca\u{B2}\u{207A}",
    "HCO\u{2083}\u{207B}",
    "Ki-67",
    "\u{3B1}-fetoprotein",
    "TNM 8\u{1D57}\u{2B0} edition",
    "\u{20B9}300",
    "\u{2022} bullet point",
    "resection \u{2014} evidence",
    "mild\u{2013}moderate pain",
    "\u{3B2}\u{2011}blocker",
    "5 \u{2013} 10 mg",
    "pros\u{B7}cons",
    "\u{A0}leading NBSP and word\u{202F}gap",
    "\u{B0} \u{B1} \u{D7} \u{F7} \u{B5} \u{2265} \u{2264} \u{2260} \u{2248} \u{223C}",
    "\u{2192} \u{2190} \u{2191} \u{2193} \u{2018}x\u{2019} \u{201C}y\u{201D} \u{2026}",
    "\u{BC} dose, \u{BE} dose, \u{AE} \u{2122} \u{A9} \u{A7}3 \u{20AC}5 \u{A3}5 3\u{2030}",
    "na\u{EF}ve, Sj\u{F6}gren, Ménétrier, \u{C6}, stra\u{DF}e",
    "\u{394}T, \u{3A3}, \u{3C0}",
    "tab\tand\nnewline\r\n",
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
    let t = "αβγ é Mrs. Lakshmi";
    let f = scan(t);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].kind, K::PersonName);
    assert_eq!(&t[f[0].start..f[0].end], "Mrs. Lakshmi");

    let t = "Ménétrier αβ 080-2502 4444 é, email a@b.in";
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
        K::UnusualCharacters,
    ] {
        assert!(!describe(k).is_empty());
    }
    assert_eq!(describe(K::Phone), "phone number");
    assert_eq!(
        describe(K::UnusualCharacters),
        "hidden or unusual characters"
    );
    assert_eq!(
        hint(K::UnusualCharacters),
        Some(
            "please retype the question without copy-pasted special characters \
             (use plain hyphens, colons and spaces)"
        )
    );
    assert_eq!(hint(K::Phone), None);
    assert_eq!(
        serde_json::to_string(&K::UnusualCharacters).unwrap(),
        "\"unusual_characters\""
    );
}

#[test]
fn unusual_characters_flag_each_maximal_run() {
    let t = "ph 98450\u{200B}\u{200C}\u{200D}12345 and 9845\u{AD}012345";
    let f = scan(t);
    assert_eq!(f.len(), 2, "{f:?}");
    assert!(f.iter().all(|f| f.kind == K::UnusualCharacters));
    assert_eq!(&t[f[0].start..f[0].end], "\u{200B}\u{200C}\u{200D}");
    assert_eq!(&t[f[1].start..f[1].end], "\u{AD}");
    // A full-width run is one finding over the whole run.
    let t = "ph \u{FF19}\u{FF18}\u{FF14} ok";
    let f = scan(t);
    assert_eq!(f.len(), 1);
    assert_eq!(&t[f[0].start..f[0].end], "\u{FF19}\u{FF18}\u{FF14}");
    // A look-alike dash is flagged only between digits.
    let t = "12\u{2010}03";
    let f = scan(t);
    assert_eq!(f.len(), 1);
    assert_eq!(&t[f[0].start..f[0].end], "\u{2010}");
}

#[test]
fn marks_only_after_latin_or_greek_letters_at_most_two() {
    assert!(scan("cafe\u{301}").is_empty());
    assert!(scan("a\u{301}\u{302}b").is_empty());
    let t = "a\u{301}\u{302}\u{303}b";
    let f = scan(t);
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(&t[f[0].start..f[0].end], "\u{303}");
    for t in ["1\u{301}", "-\u{301}", "\u{301}a", " \u{301}"] {
        assert_eq!(kinds(t), [K::UnusualCharacters], "{t:?}");
    }
}

#[test]
fn contextual_characters_only_away_from_digits() {
    for c in [
        '\u{2013}', '\u{2014}', '\u{2010}', '\u{2011}', '\u{2212}', '\u{B7}', '\u{A0}', '\u{202F}',
    ] {
        assert!(scan(&format!("a{c}b")).is_empty(), "U+{:04X}", c as u32);
        assert!(scan(&format!("a {c} b")).is_empty(), "U+{:04X}", c as u32);
        for t in [format!("1{c}a"), format!("a{c}1"), format!("1{c}1")] {
            assert_eq!(kinds(&t), [K::UnusualCharacters], "{t:?}");
        }
    }
}

#[test]
fn superscripts_extending_a_number_are_flagged() {
    assert_eq!(
        kinds("ph 98450\u{B9}\u{B2}\u{B3}\u{2074}\u{2075}"),
        [K::UnusualCharacters]
    );
    assert_eq!(kinds("m\u{B2}5"), [K::UnusualCharacters]);
}

#[test]
fn every_format_character_is_flagged() {
    // Every Unicode format (Cf) character, not a hand-picked list.
    for c in [
        '\u{AD}',
        '\u{600}',
        '\u{61C}',
        '\u{6DD}',
        '\u{70F}',
        '\u{180E}',
        '\u{200B}',
        '\u{200C}',
        '\u{200D}',
        '\u{200E}',
        '\u{200F}',
        '\u{202A}',
        '\u{202B}',
        '\u{202C}',
        '\u{202D}',
        '\u{202E}',
        '\u{2060}',
        '\u{2061}',
        '\u{2062}',
        '\u{2063}',
        '\u{2064}',
        '\u{2066}',
        '\u{2069}',
        '\u{206F}',
        '\u{FEFF}',
        '\u{FFF9}',
        '\u{FFFB}',
        '\u{110BD}',
        '\u{1D173}',
        '\u{E0001}',
        '\u{E0041}',
        '\u{E007F}',
    ] {
        let t = format!("a{c}b");
        assert_eq!(kinds(&t), [K::UnusualCharacters], "U+{:04X}", c as u32);
    }
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
        ("NAME: RAMAKKA AGE/SEX: 45/F", "NAME: RAMAKKA"),
        ("PATIENT NAME: RAMAKKA", "PATIENT NAME: RAMAKKA"),
    ] {
        let f = scan(t);
        assert_eq!(&t[f[0].start..f[0].end], want, "{t}");
    }
}

/// Every probe from fix round 4 (R-2d), with its expected kinds (empty = must pass).
/// Checked together with every BLOCK and PASS row so no earlier row can regress.
const ROUND4_PROBES: &[(&str, &[K])] = &[
    // 1. PIN followed by a verb
    ("from Whitefield 560066 presents with pain", &[K::Address]),
    (
        "Address: Whitefield, Bengaluru 560066 presents for review",
        &[K::Address],
    ),
    ("Kadugodi, Bengaluru 560067 visits OPD", &[K::Address]),
    ("Bengaluru 560038 attends clinic", &[K::Address]),
    // 2. Lt / Late
    ("c/o Lt Inguinal Swelling", &[]),
    ("c/o Lt Sided Abdominal Pain", &[]),
    ("s/o Lt Inguinal Hernia", &[]),
    ("Healed w/o Late Complications", &[]),
    ("s/o Late Sri Ramaiah", &[K::PersonName]),
    ("s/o late Ramaiah", &[K::PersonName]),
    // 3. s/o + clinical words
    ("CT s/o Intestinal Obstruction", &[]),
    ("S/O INTESTINAL OBSTRUCTION", &[]),
    ("s/o Achalasia (Cardia)", &[]),
    ("s/o Duodenal Ulcer Perforation", &[]),
    ("s/o Intussusception", &[]),
    ("s/o Rectal Growth", &[]),
    ("s/o Umbilical Hernia", &[]),
    ("s/o Perianal Fistula", &[]),
    ("s/o Peritoneal Carcinomatosis", &[]),
    ("Findings s/o Sigmoid Volvulus", &[]),
    ("s/o Hepatic Abscess", &[]),
    ("s/o Pancreatic Pseudocyst", &[]),
    ("s/o Gastric Outlet Obstruction", &[]),
    ("s/o Appendicular Lump", &[]),
    ("s/o Mesenteric Ischemia", &[]),
    ("s/o Koch's Abdomen", &[]),
    ("s/o Free Fluid", &[]),
    ("S/O SUBACUTE INTESTINAL OBSTRUCTION", &[]),
    ("s/o Gas Under Diaphragm", &[]),
    ("s/o Fatty Liver", &[]),
    ("s/o Calculous Cholecystitis", &[]),
    ("S/o Gopal", &[K::PersonName]),
    ("s/o Vishal", &[K::PersonName]),
    ("s/o Kunal", &[K::PersonName]),
    ("s/o Sonal", &[K::PersonName]),
    ("w/o Kamal", &[K::PersonName]),
    ("D/o Sonia", &[K::PersonName]),
    ("S/O. GOPAL", &[K::PersonName]),
    ("S/O GOPAL KRISHNA", &[K::PersonName]),
    ("s/o Kunal, 23M", &[K::PersonName]),
    // 4. second word must be a name
    ("h/o Previous Surgery", &[]),
    ("h/o Similar Complaints", &[]),
    ("c/o Heaviness Over Left Groin", &[]),
    ("Consent from Son of Patient", &[]),
    ("Wife of Patient informed", &[]),
    // 5. durations are not ages
    ("h/o Smoking 20 years", &[]),
    ("h/o Alcohol Intake 15 years", &[]),
    ("h/o Surgery 10 yrs ago", &[]),
    ("h/o Asthma 20 years", &[]),
    ("h/o Tobacco Chewing 30 years", &[]),
    ("h/o Trauma 12 years back", &[]),
    ("h/o Ramesh, 60M", &[K::PersonName]),
    ("c/o Kamala, aged 45", &[K::PersonName]),
    ("MRS SAROJAMMA, 67 F", &[K::PersonName]),
    // 6. ranges and pH
    ("ABG pH 7.31, Plt 65000-80000", &[]),
    // 7. units
    ("Flat 8 MM polyp", &[]),
    ("Flat 2 CM lesion", &[]),
    ("IP 500 MG CISPLATIN", &[]),
    ("IP 1 g ceftriaxone", &[]),
    ("HBV DNA 7800000000 IU/mL", &[]),
    ("House No. 45 G Block", &[K::Address]),
    ("Door No 12 G Floor", &[K::Address]),
    ("Flat 302 G Wing, Prestige", &[K::Address]),
    ("IP 4471 M 45y", &[K::HospitalId]),
    ("IP 4471 m", &[K::HospitalId]),
    // 9. optional
    ("call on 98450 12 345", &[K::Phone]),
    ("s/o Om", &[K::PersonName]),
    // --- round 5 probes
    ("S/O MUNIYAPPA OCCUPATION: AGRICULTURIST", &[K::PersonName]),
    (
        "S/O NAGESH GOWDA DIAGNOSIS: RT INGUINAL HERNIA",
        &[K::PersonName],
    ),
    ("D/O SHIVALINGAPPA DIAGNOSIS CA BREAST", &[K::PersonName]),
    ("s/o Venkatappa Station Road, Hoskote", &[K::PersonName]),
    (
        "s/o Thimmaiah Admission Diagnosis Acute Appendicitis",
        &[K::PersonName],
    ),
    ("Son of Muniswamy Diagnosis: SAIO", &[K::PersonName]),
    (
        "Wife of Chandrashekar Occupation Housewife",
        &[K::PersonName],
    ),
    ("s/o Ramappa Station Road", &[K::PersonName]),
    ("c/o Puttaswamy, 56 yrs", &[K::PersonName]),
    ("c/o Puttaswamy, 56 Years, Male", &[K::PersonName]),
    ("w/o Kariyappa, 48 yrs", &[K::PersonName]),
    ("Patient Nancy admitted", &[K::PersonName]),
    ("Pt Soma, 45F", &[K::PersonName]),
    ("Patient Roma Devi", &[K::PersonName]),
    ("Name: Nancy Thomas", &[K::PersonName]),
    ("s/o Sebaceous Cyst", &[]),
    ("s/o Dermoid Cyst", &[]),
    ("s/o Intestinal Malrotation", &[]),
    ("s/o Arteriovenous Malformation", &[]),
    ("s/o Paget Disease", &[]),
    ("S/O CA RECTUM", &[]),
    ("Impression: s/o Acute Appendicitis", &[]),
    ("s/o Carcinoid Tumour", &[]),
    ("s/o Ovarian Torsion", &[]),
    ("c/o Giddiness, 2 years", &[]),
    ("c/o Pain Abdomen, 10 years", &[]),
    ("c/o Swelling, 10 years duration", &[]),
    ("c/o Dyspepsia, 12 years", &[]),
    ("c/o Heartburn, 15 years back", &[]),
    ("h/o Epilepsy, 12 yrs", &[]),
    ("CT s/o Annular Pancreas", &[]),
    // --- round 6 probes
    (
        "s/o Thimmaiah Diagnosis Acute Appendicitis",
        &[K::PersonName],
    ),
    ("S/O MUNIYAPPA DIAGNOSIS INGUINAL HERNIA", &[K::PersonName]),
    ("s/o Muniyappa Diagnosis Hernia", &[K::PersonName]),
    (
        "S/O MUNIYAPPA IMPRESSION ACUTE APPENDICITIS",
        &[K::PersonName],
    ),
    (
        "Wife of Chandrashekar Diagnosis Carcinoma",
        &[K::PersonName],
    ),
    (
        "D/O Venkatappa Diagnosis Carcinoma Breast",
        &[K::PersonName],
    ),
    ("S/O MUNIYAPPA WITH INGUINAL HERNIA", &[K::PersonName]),
    ("Impression: s/o Acute Appendicitis", &[]),
    ("s/o Sigmoid Diverticulitis with pericolic abscess", &[]),
    (
        "USG s/o Acute Calculous Cholecystitis with GB wall edema",
        &[],
    ),
    ("s/o Desmoid Tumour of anterior abdominal wall", &[]),
    ("c/o Puttaswamy, 56 years of age", &[K::PersonName]),
    ("W/O MUNIRAJU, 54 years of age", &[K::PersonName]),
    (
        "c/o Puttaswamy , 56 years, history of hernia",
        &[K::PersonName],
    ),
    ("c/o Dyspepsia, 12 years of history", &[]),
    ("c/o Pain, 2 months of duration", &[]),
    ("NAME: RAMAKKA", &[K::PersonName]),
    ("Name: RAMAKKA", &[K::PersonName]),
    ("NAME: Ramakka", &[K::PersonName]),
    ("NAME : RAMAKKA", &[K::PersonName]),
    ("PATIENT NAME: RAMAKKA", &[K::PersonName]),
    ("NAME: RAMESH", &[K::PersonName]),
    ("NAME: RAMAKKA AGE/SEX: 45/F", &[K::PersonName]),
    ("Name of the procedure: Whipple", &[]),
    ("NAME OF SURGERY: LAP CHOLECYSTECTOMY", &[]),
    ("Brand name: Prolene mesh", &[]),
    ("Drug name: Tranexamic acid", &[]),
    ("Trial name: ROLARR", &[]),
    ("w/o Recurrence, 10 years later", &[]),
    ("c/o Backache, 10 yrs", &[]),
    ("Biopsy s/o Signet Ring Cell Carcinoma", &[]),
];

#[test]
fn regression_corpus() {
    let mut bad = Vec::new();
    let blocks = BLOCK.iter().copied();
    let passes = PASS.iter().map(|t| (*t, &[][..]));
    for (text, want) in blocks.chain(passes).chain(ROUND4_PROBES.iter().copied()) {
        let got = kinds(text);
        if got != want {
            bad.push(format!("{text:?}: want {want:?}, got {got:?}"));
        }
    }
    assert!(bad.is_empty(), "regressions:\n{}", bad.join("\n"));
}
