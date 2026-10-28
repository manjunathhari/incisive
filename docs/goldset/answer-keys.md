<!-- Owner-supplied answer keys (28 Oct 2026), kept verbatim below.
Reference check, 28 Oct 2026 (PubMed): all 20 PMIDs listed resolve, and their DOIs and titles match,
including the four marked "verify" (42929021, 42814447, 42565915, and 10.1093/bjs/znad435 = PMID 38215239).
The verification covered metadata only. The numbers in the answer keys were not re-checked against the papers.
Q10-Q12 landmarks added in questions.toml (pending owner confirmation):
  Q10: EUROPA PMID 38420108; Liu et al. PMID 38428441; PORTAL PMID 42386316
  Q11: OSLO-COMET PMID 28657937; propensity-matched robotic vs laparoscopic MA PMID 40164991
  Q12: EUROPA 38420108; DIPLOMA 37457332; major hepatectomy MA PMID 32989544 -->

# Surgical Research LLM Benchmark — Questions and Gold-Standard Answer Keys

**Scope:** Robotic GI, colorectal, general, and HPB surgery  
**Format:** 20 benchmark questions with concise answer keys and source links  
**Important:** These answer keys are scoring guides, not substitutes for checking the original papers. Verify citation metadata, endpoints, and numerical results against the full publications before freezing this as a formal gold-standard dataset. Where evidence is not firmly established, a correct answer should say so.

---

## Q1. Does robotic surgery reduce conversion to open surgery compared with conventional laparoscopy in rectal cancer?

**Question**

In patients undergoing curative resection for rectal cancer, does robotic-assisted surgery significantly reduce conversion to open surgery compared with conventional laparoscopy? Discuss the primary endpoint, absolute conversion rates, statistical significance, and clinical implications.

**Gold-standard answer**

In the ROLARR randomized clinical trial, conversion to open surgery occurred in approximately 8.1% of robotic cases and 12.2% of laparoscopic cases. The difference was not statistically significant. The trial therefore did not establish that robotic surgery significantly reduces conversion for the overall study population. The result does not prove the approaches are identical, nor does it exclude a possible benefit in selected subgroups or experienced hands. Answers should identify the trial design, report the result accurately, and avoid claiming that robotics categorically prevents conversion.

**References**

- Jayne et al. ROLARR randomized clinical trial. DOI: https://doi.org/10.1001/jama.2017.7219 | PMID: https://pubmed.ncbi.nlm.nih.gov/29067426/
- Collinson et al. Randomized trial of robotic-assisted versus standard laparoscopic surgery for rectal cancer. DOI: https://doi.org/10.1007/s00384-011-1313-6 | PMID: https://pubmed.ncbi.nlm.nih.gov/21912876/

---

## Q2. Does robotic total mesorectal excision improve oncological outcomes compared with laparoscopic surgery?

**Question**

Compare robotic and laparoscopic total mesorectal excision (TME) for middle and low rectal cancer. What does the evidence show regarding circumferential resection margin positivity, completeness of TME, postoperative complications, and long-term oncological outcomes?

**Gold-standard answer**

The answer should synthesize randomized evidence, including REAL and COLRAR, while distinguishing short-term surgical and pathological outcomes from long-term recurrence and survival. It should report the endpoints and results from each trial separately, avoid treating the trials as interchangeable, and explain that better performance on a particular short-term endpoint does not automatically establish a long-term survival advantage. The final conclusion must follow the verified trial results and their confidence intervals, population, and follow-up.

**References**

- Feng et al. REAL randomized controlled trial. DOI: https://doi.org/10.1016/S2468-1253(22)00248-5 | PMID: https://pubmed.ncbi.nlm.nih.gov/36087608/
- Park et al. COLRAR randomized controlled trial. DOI: https://doi.org/10.1097/SLA.0000000000005788 | PMID: https://pubmed.ncbi.nlm.nih.gov/36594748/
- Reported long-term COLRAR publication: DOI: https://doi.org/10.1093/bjs/znag114 | PMID: https://pubmed.ncbi.nlm.nih.gov/42929021/ **(verify metadata and results before use)**

---

## Q3. Does robotic colorectal surgery provide better functional outcomes?

**Question**

Does robotic rectal cancer surgery improve postoperative urinary function, sexual function, bowel function, or quality of life compared with laparoscopic surgery? Evaluate evidence strength and distinguish statistical significance from clinically meaningful benefit.

**Gold-standard answer**

A strong answer identifies which functional outcomes were actually measured, the assessment tools and follow-up times, and whether differences were statistically and clinically meaningful. It should discuss the rationale for improved pelvic autonomic nerve preservation without assuming that a plausible mechanism proves a patient benefit. Conclusions must reflect the individual trials; robotic surgery should not be described as universally improving urinary, sexual, or bowel function unless the evidence supports that specific claim.

**References**

- ROLARR trial. DOI: https://doi.org/10.1001/jama.2017.7219
- REAL trial. DOI: https://doi.org/10.1016/S2468-1253(22)00248-5
- COLRAR trial. DOI: https://doi.org/10.1097/SLA.0000000000005788

---

## Q4. Is the learning curve for robotic colorectal surgery shorter than that for laparoscopy?

**Question**

How many cases are required to achieve proficiency in robotic colorectal surgery? Compare learning curves for robotic and laparoscopic colectomy and rectal surgery, considering prior experience, procedure complexity, and how proficiency is defined.

**Gold-standard answer**

There is no single universal case number that establishes proficiency across all robotic colorectal procedures and surgeons. Published learning curves use different outcomes, including operative time, conversion, complications, and composite proficiency measures. A sound answer should distinguish technical efficiency from safe independent practice and oncological competence, discuss heterogeneity and prior laparoscopic experience, and explain why operative time alone is an inadequate proficiency measure.

**References**

- Flynn et al. Systematic review of the learning curve in robotic versus laparoscopic colorectal surgery. DOI: https://doi.org/10.1111/codi.15843 | PMID: https://pubmed.ncbi.nlm.nih.gov/34318575/
- Bhattacharya et al. “Learning Curve in Robotic Colorectal Surgery.” DOI: https://doi.org/10.4293/JSLS.2025.00077 | PMID: https://pubmed.ncbi.nlm.nih.gov/41334488/
- Nocera et al. Learning curves in robotic and laparoscopic colorectal surgery. DOI: https://doi.org/10.1007/s11701-026-04024-x | PMID: https://pubmed.ncbi.nlm.nih.gov/42828702/

---

## Q5. Does robotic gastrectomy improve outcomes compared with laparoscopic gastrectomy for gastric cancer?

**Question**

Does robotic gastrectomy improve postoperative morbidity, lymph-node retrieval, margin status, or long-term survival compared with laparoscopic gastrectomy?

**Gold-standard answer**

The answer should separate perioperative outcomes, pathological quality, and long-term oncological outcomes. It should identify whether each cited study is randomized or observational and consider selection bias, surgeon experience, and institutional volume. Technical advantages or differences in lymph-node yield do not by themselves establish improved survival. Any claim of oncological superiority must be supported by appropriate comparative evidence and follow-up.

**References**

- Kim et al. Multicenter prospective comparative study of robotic versus laparoscopic gastrectomy for gastric adenocarcinoma. DOI: https://doi.org/10.1097/SLA.0000000000001249 | PMID: https://pubmed.ncbi.nlm.nih.gov/26020107/
- “Long-term oncological outcomes of robotic versus laparoscopic gastrectomy for gastric cancer: multicentre cohort study.” DOI: https://doi.org/10.1093/bjs/znad435 **(verify exact metadata and findings)**

---

## Q6. What is the evidence supporting minimally invasive gastrectomy for locally advanced gastric cancer?

**Question**

How does laparoscopic gastrectomy compare with open gastrectomy for locally advanced gastric cancer in complications, recurrence, disease-free survival, and overall survival? What are the implications for choosing a minimally invasive approach?

**Gold-standard answer**

The answer should accurately summarize the relevant KLASS-02 randomized trial outcomes and distinguish laparoscopic-versus-open evidence from robotic-versus-laparoscopic evidence. It should interpret noninferiority or survival findings according to the actual trial design and endpoint. Results from distal gastrectomy should not automatically be generalized to total gastrectomy or every gastric cancer population.

**References**

- Son et al. KLASS-02-RCT long-term outcomes. DOI: https://doi.org/10.1001/jamasurg.2022.2749 | PMID: https://pubmed.ncbi.nlm.nih.gov/35857305/
- KLASS-02 randomized trial long-term outcome literature. PMID: https://pubmed.ncbi.nlm.nih.gov/32816629/

---

## Q7. Does robotic esophagectomy improve lymph-node retrieval and perioperative outcomes?

**Question**

Compare robot-assisted minimally invasive esophagectomy with conventional minimally invasive esophagectomy for esophageal cancer. What are the effects on lymph-node yield, complications, margin status, mortality, and survival?

**Gold-standard answer**

The answer must identify the comparator, trial design, population, and prespecified primary endpoint before interpreting results. Lymph-node yield, complication rates, margin status, and survival are distinct outcomes. It should discuss uncertainty, follow-up, and generalizability from specialist centers. Claims about a specific 2026 randomized trial should only be made after verifying the publication and results.

**References**

- Tagkalos et al. ROBOT-2 randomized controlled trial. DOI: https://doi.org/10.1186/s12885-021-08780-x | PMID: https://pubmed.ncbi.nlm.nih.gov/34565343/
- Reported 2026 randomized trial: DOI: https://doi.org/10.1001/jamasurg.2026.4478 | PMID: https://pubmed.ncbi.nlm.nih.gov/42814447/ **(verify metadata and results before use)**

---

## Q8. Is robotic distal pancreatectomy superior to laparoscopic distal pancreatectomy?

**Question**

Compare robotic and laparoscopic distal pancreatectomy for benign, premalignant, and malignant lesions. Evaluate conversion, operative time, blood loss, postoperative pancreatic fistula, spleen preservation, margin status, and oncological outcomes.

**Gold-standard answer**

A good answer distinguishes observational comparisons from randomized evidence and reports outcomes separately rather than treating “better outcomes” as one endpoint. It should consider case selection, surgeon and center volume, disease type, and definitions of postoperative pancreatic fistula. Pooled observational associations should not be presented as proof of causation or as evidence of universal robotic superiority.

**References**

- Kamarajah et al. Systematic review and meta-analysis of robotic versus laparoscopic distal pancreatic resection. DOI: https://doi.org/10.1016/j.hpb.2019.02.020 | PMID: https://pubmed.ncbi.nlm.nih.gov/30962137/
- Sabah et al. Updated systematic review and meta-analysis of robotic versus laparoscopic distal pancreatectomy. DOI: https://doi.org/10.1007/s11701-026-03464-9 | PMID: https://pubmed.ncbi.nlm.nih.gov/42133237/

---

## Q9. Is minimally invasive distal pancreatectomy oncologically safe for resectable pancreatic cancer?

**Question**

Does minimally invasive distal pancreatectomy provide oncological outcomes comparable to open surgery for resectable pancreatic cancer? Explain the primary endpoint, noninferiority framework, R0 resection, and the limits of applying results specifically to robotic surgery.

**Gold-standard answer**

The answer should identify DIPLOMA as a randomized noninferiority trial and report its actual primary endpoint, noninferiority margin, and findings from the paper. It must explain that noninferiority means the results fell within a prespecified margin for the studied endpoint; it does not prove superiority or equivalence on every outcome. If the study combines laparoscopic and robotic approaches, it cannot automatically establish robotic-specific superiority.

**Reference**

- DIPLOMA trial. “Minimally invasive versus open distal pancreatectomy for resectable pancreatic cancer: an international randomised non-inferiority trial.” DOI: https://doi.org/10.1016/j.lanepe.2023.100673 | PMID: https://pubmed.ncbi.nlm.nih.gov/37457332/

---

## Q10. What evidence supports robotic pancreaticoduodenectomy for pancreatic and periampullary malignancies?

**Question**

Evaluate robotic pancreaticoduodenectomy compared with open pancreaticoduodenectomy. Discuss morbidity, clinically relevant pancreatic fistula, delayed gastric emptying, postpancreatectomy hemorrhage, conversion, length of stay, R0 resection, lymph-node yield, and mortality.

**Gold-standard answer**

The answer should prioritize directly relevant comparative studies and clearly label their design. It must not conflate laparoscopic pancreaticoduodenectomy with robotic pancreaticoduodenectomy. It should discuss learning curves, center volume, patient selection, and standardized complication definitions. If high-quality randomized evidence is limited for the precise comparison, the answer should say so and distinguish feasibility or observational outcomes from proven superiority.

**Reference searches to curate**

- PADULAP trial literature: https://pubmed.ncbi.nlm.nih.gov/?term=PADULAP+pancreaticoduodenectomy+randomized+trial
- Robotic versus open pancreaticoduodenectomy randomized trials: https://pubmed.ncbi.nlm.nih.gov/?term=robotic+versus+open+pancreaticoduodenectomy+randomized+trial

**Gold-standard curation needed:** Select and verify 1–3 directly relevant papers before scoring.

---

## Q11. Is robotic liver resection superior to laparoscopic or open liver resection?

**Question**

For benign or malignant liver disease, compare robotic, laparoscopic, and open liver resection for blood loss, transfusion, conversion, morbidity, length of stay, margin status, and long-term oncological outcomes. How might selection and institutional expertise affect the findings?

**Gold-standard answer**

The answer should distinguish evidence comparing minimally invasive surgery with open surgery from evidence directly comparing robotic with laparoscopic surgery. It should account for tumor location, extent of resection, underlying liver disease, and selection bias. Evidence from minor resections should not automatically be generalized to major hepatectomy. Perioperative advantages do not by themselves establish oncological superiority.

**Reference searches to curate**

- Minimally invasive versus open liver resection: https://pubmed.ncbi.nlm.nih.gov/?term=minimally+invasive+versus+open+liver+resection+randomized+trial
- Robotic versus laparoscopic liver resection: https://pubmed.ncbi.nlm.nih.gov/?term=robotic+versus+laparoscopic+liver+resection+systematic+review

**Gold-standard curation needed:** Select procedure-specific landmark papers before scoring.

---

## Q12. How should surgeons interpret evidence for robotic surgery in complex HPB procedures?

**Question**

For major hepatectomy, bile duct reconstruction, and complex pancreatic surgery, how should surgeons decide whether robotic surgery provides a clinically meaningful advantage? Discuss complexity, learning curves, conversion, complications, oncological endpoints, and institutional experience.

**Gold-standard answer**

A strong answer evaluates each procedure separately and distinguishes feasibility, safety, and superiority. It should identify gaps in procedure-specific evidence and explain the limitations of observational series from high-volume centers. It should not extrapolate findings from distal pancreatectomy to pancreaticoduodenectomy or from minor liver resection to major hepatectomy.

**References**

- Kamarajah et al. Robotic versus laparoscopic distal pancreatic resection meta-analysis. DOI: https://doi.org/10.1016/j.hpb.2019.02.020
- DIPLOMA trial. DOI: https://doi.org/10.1016/j.lanepe.2023.100673
- Robotic versus laparoscopic liver resection literature: https://pubmed.ncbi.nlm.nih.gov/?term=robotic+versus+laparoscopic+liver+resection

**Gold-standard curation needed:** Add direct evidence for each specific procedure before scoring.

---

## Q13. Does robotic inguinal hernia repair improve outcomes compared with laparoscopic repair?

**Question**

Does robotic inguinal hernia repair improve pain, complications, recurrence, return to normal activity, or quality of life compared with conventional laparoscopic repair? Assess randomized evidence, operative time, and costs.

**Gold-standard answer**

The answer should identify the RIVAL randomized clinical pilot trial and accurately describe the outcomes it measured. RIVAL did not establish a significant improvement in the measured short-term clinical outcomes with robotics. Operative time and costs are relevant, and the trial should not be generalized to every complex, recurrent, or unusual hernia presentation.

**Reference**

- Prabhu et al. RIVAL randomized clinical trial. DOI: https://doi.org/10.1001/jamasurg.2020.0034 | PMID: https://pubmed.ncbi.nlm.nih.gov/32186683/

---

## Q14. Does robotic ventral hernia repair offer better outcomes than laparoscopic repair?

**Question**

For ventral hernia repair using intraperitoneal mesh, compare robotic and laparoscopic approaches in complications, operative time, pain, recurrence, mesh-related outcomes, length of stay, and costs.

**Gold-standard answer**

The answer should identify the PROVE-IT randomized trial and its specific repair technique and mesh placement. It should report the trial's actual outcomes and follow-up, avoid generalizing to all abdominal wall reconstruction techniques, and distinguish technical or ergonomic benefits from demonstrated patient benefits. Costs and operative time should be considered.

**Reference**

- Petro et al. PROVE-IT randomized clinical trial, 1-year exploratory outcomes. DOI: https://doi.org/10.1097/XCS.0000000000000171 | PMID: https://pubmed.ncbi.nlm.nih.gov/35703814/

---

## Q15. Does robotic right colectomy improve outcomes compared with conventional laparoscopy?

**Question**

Compare robotic and laparoscopic right colectomy for benign and malignant disease. Discuss operative time, conversion, complications, length of stay, intracorporeal anastomosis, recovery, and oncological quality.

**Gold-standard answer**

The answer should identify the population and methods of the randomized study and report its measured outcomes accurately. It should not assume that technical advantages or intracorporeal anastomosis necessarily prove a patient-centered benefit. It should consider surgeon experience and note that older trials may not fully represent current platforms or enhanced recovery practice.

**Reference**

- Park et al. Randomized clinical trial of robot-assisted versus standard laparoscopic right colectomy. DOI: https://doi.org/10.1002/bjs.8841 | PMID: https://pubmed.ncbi.nlm.nih.gov/22864881/

---

## Q16. Is robotic surgery cost-effective compared with laparoscopic surgery?

**Question**

Across colorectal, upper GI, HPB, and general surgery, is robotic surgery cost-effective compared with conventional laparoscopy? Evaluate capital expenditure, instruments, operating-room time, length of stay, complications, readmissions, quality-adjusted life-years, and procedure volume.

**Gold-standard answer**

There is no single universal cost-effectiveness conclusion across all procedures, health systems, and case volumes. A good answer specifies the perspective—hospital, payer, or societal—and distinguishes cost minimization from cost-effectiveness. It should account for capital and maintenance costs, instruments, operating-room time, utilization, case mix, complications, and downstream costs. It should not assert that robotics is always or never cost-effective.

**References**

- ROLARR trial. DOI: https://doi.org/10.1001/jama.2017.7219
- RIVAL trial. DOI: https://doi.org/10.1001/jamasurg.2020.0034
- Reported umbrella review on robotic colorectal surgery economics. DOI: https://doi.org/10.1007/s11701-026-03716-8 | PMID: https://pubmed.ncbi.nlm.nih.gov/42565915/ **(verify metadata and findings)**

---

## Q17. How should a surgical team establish a robotic surgery program safely?

**Question**

Design an evidence-based implementation pathway for a new robotic GI and colorectal surgery program. Address credentialing, simulation, proctoring, case selection, learning curves, complication monitoring, conversion thresholds, outcome auditing, and expansion to complex HPB procedures.

**Gold-standard answer**

A safe pathway should use structured training, procedure-specific credentialing, proctoring where appropriate, careful initial case selection, and ongoing audit. Progression should be based on objective technical and clinical outcomes rather than one arbitrary case count. Monitoring should include conversion, complications, reoperation, readmission, mortality, and relevant procedure-specific endpoints, with risk adjustment where possible. Competence in one operation should not automatically confer competence in another.

**References**

- Flynn et al. Learning-curve systematic review. DOI: https://doi.org/10.1111/codi.15843
- Bhattacharya et al. “Learning Curve in Robotic Colorectal Surgery.” DOI: https://doi.org/10.4293/JSLS.2025.00077
- Nocera et al. Learning-curves systematic review. DOI: https://doi.org/10.1007/s11701-026-04024-x

---

## Q18. How should an LLM interpret conflicting randomized trials of robotic surgery?

**Question**

Suppose one randomized trial shows improved short-term outcomes with robotic rectal surgery, another shows no significant difference, and a third reports improved long-term oncological outcomes. How should the evidence be synthesized?

**Gold-standard answer**

The system should compare populations, intervention and comparator details, surgeon experience, primary endpoints, statistical power, effect estimates, confidence intervals, and follow-up. It should distinguish statistical significance from clinical importance and avoid cherry-picking. Pooling is appropriate only when the studies are sufficiently comparable and the synthesis method is justified. Conflicting results should be explained, not hidden.

**References**

- ROLARR. DOI: https://doi.org/10.1001/jama.2017.7219
- REAL. DOI: https://doi.org/10.1016/S2468-1253(22)00248-5
- COLRAR. DOI: https://doi.org/10.1097/SLA.0000000000005788

---

## Q19. How should robotic surgery studies report and compare postoperative complications?

**Question**

When comparing robotic and laparoscopic GI surgery, which complication definitions and outcome measures should be used? Explain Clavien–Dindo grading, 30- and 90-day morbidity, readmission, reoperation, mortality, conversion, pancreatic fistula, and anastomotic leak.

**Gold-standard answer**

Studies should use explicit, consistent definitions and follow-up windows. Clavien–Dindo grading distinguishes complication severity, and reporting should separate all-grade from major complications. Thirty- and 90-day outcomes can capture different events. Procedure-specific definitions should be used for outcomes such as pancreatic fistula and anastomotic leak. Comparisons are unreliable if surveillance, definitions, or reporting periods differ.

**References**

- ROLARR. DOI: https://doi.org/10.1001/jama.2017.7219
- DIPLOMA. DOI: https://doi.org/10.1016/j.lanepe.2023.100673
- RIVAL. DOI: https://doi.org/10.1001/jamasurg.2020.0034

---

## Q20. How can published robotic surgery evidence be translated into an individualized recommendation?

**Question**

A patient with low rectal cancer, obesity, previous abdominal surgery, and significant comorbidity is being considered for robotic versus laparoscopic resection. How should published evidence guide the decision? Discuss trial applicability, patient factors, surgeon expertise, anticipated conversion, oncological objectives, and uncertainty.

**Gold-standard answer**

The answer should apply trial evidence cautiously, assess how closely the patient resembles the studied populations, and consider anatomy, comorbidity, prior surgery, surgeon-specific experience, and institutional outcomes. It should discuss trade-offs and uncertainty rather than claiming one approach is universally superior. Population-level trial results are not a validated individual risk estimate unless an appropriate prediction model has been developed and validated for that use.

**References**

- ROLARR. DOI: https://doi.org/10.1001/jama.2017.7219
- REAL. DOI: https://doi.org/10.1016/S2468-1253(22)00248-5
- COLRAR. DOI: https://doi.org/10.1097/SLA.0000000000005788

---

## Minimum scoring criteria

For each answer, score:

- **Factual accuracy:** correct trial design, population, endpoints, results, and conclusion.
- **Reference quality:** retrieves the most relevant papers, not merely papers sharing keywords.
- **Citation integrity:** title, DOI, PMID, and linked source agree.
- **Critical appraisal:** recognizes bias, limitations, uncertainty, and indirectness.
- **Clinical interpretation:** separates statistical significance from clinical importance and avoids unsupported recommendations.

Flag critical errors separately: fabricated or mismatched citations, reversed results, incorrect primary endpoints, observational evidence represented as randomized, or unsupported claims of oncological superiority.
