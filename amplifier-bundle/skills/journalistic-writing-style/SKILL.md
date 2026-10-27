---
name: journalistic-writing-style
description: "A journalistic writing style for correcting long-form text produced by language models. Use it when drafting, editing or reviewing any document where clarity, simplicity and neutrality are wanted: reports, memos, research notes, engineering plans, README files, status files and conversational replies. For the structure, placement or Diataxis type of software documentation, use documentation-writing instead."
---

# Journalistic writing style

This skill corrects long-form text produced by language models so that it reads as clear, simple and neutral prose. It applies to any document where those three qualities are wanted, and to conversational replies as well as to documents. It is a specific list of faults to check for, with the reason each one fails and what to do instead, followed by a reading procedure to run before a document is finished.

Keep positive descriptions precise and short. Use a neutral, journalistic and technical tone, but avoid technical jargon and use lay terms to explain concepts where possible. Use prose that describes the state of the world as a well-informed reporter would, in formal British English, in proper sentences, with plain words where plain words exist, and with humility in authorship.

The Economist's writing style is the example aspired to. Its current guide is [Writing with Style: The Economist Guide](https://profilebooks.com/work/writing-with-style/) by Lane Greene (Profile Books, 2023), whose publication the newspaper announced in [an article of 25 January 2024](https://www.economist.com/news/2024/01/25/shock-horror-the-economist-has-updated-its-guide-to-good-writing); the earlier [twelfth edition of The Economist Style Guide](https://profilebooks.com/work/the-economist-style-guide/) (Profile Books, 2018; [the publisher's preview of its opening chapters](https://profilebooks.com/wp-content/uploads/wpallimport/files/PDFs/9781781258316_preview.pdf)) opens with the instruction to think what you want to say and then say it as simply as possible. It quotes Orwell's six rules from "Politics and the English Language" (1946): never use a metaphor, simile or other figure of speech which you are used to seeing in print; never use a long word where a short one will do; if it is possible to cut a word out, always cut it out; never use the passive where you can use the active; never use a foreign phrase, a scientific word or a jargon word if you can think of an everyday English equivalent; break any of these rules sooner than say anything outright barbarous. To these it adds its own: do not be stuffy, do not be hectoring or arrogant, do not be too pleased with yourself, do not be too chatty, do not be too didactic, and do your best to be lucid. Everything below is a list of ways that standard is departed from, and how to return to it.

## 1. Structural faults

These matter more than word choice. A document can pass every word-level check and still read as generated if its shape is wrong.

The commonest fault is a bulleted list presented as paragraphs: each paragraph opens with a bolded topic sentence, follows it with one or two sentences of justification, and the pattern repeats identically for every item in a section. The rhythm never varies, and that uniformity is the strongest single sign that a machine produced the text. Fold the main idea into the paragraph and vary where it falls, sometimes first, sometimes as the paragraph's conclusion, sometimes in the middle with evidence either side. No two consecutive paragraphs should share the same shape. Bold is reserved for a defined term or a phrase that must stand out once.

The same fault appears at section scale when every section is built as a statement of the point, one paragraph of reasoning, and one sentence naming the next action. If the sections of a document could be swapped without anyone noticing, the shape is mechanical. Let sections differ in length and construction according to what each has to say.

A heading is a label for its section. Rhetorical-question headings ("Who pays for this today?"), headings that assert a conclusion, generic function headings that would fit any memo ("Key Findings", "What This Means", "Open Questions", "Where this leaves us") and any count in a heading ("Five reasons") are all to be avoided. The test is to read the headings alone, in sequence, with no body text. If they could head any strategy memo, or read as a list of debating points, rewrite them as short noun phrases naming the subject of each section.

Do not end a paragraph or section on a summing-up line. The short, weighty sentence or fragment placed at the end to serve as a summing-up, the antithetical epigram ("Neither is urgent for a demonstration; both are urgent for a contract") and the line that turns the preceding argument into a slogan are all the same reflex. Once the point has been made, stop; anything still needed belongs in the body. A sentence-fragment list used once, deliberately, for emphasis is permitted.

The text must never describe its own construction, revision or purpose in its main body. "This document collects", "this note asks", "none is answered here", "an earlier draft", "this was overstated above", "now states", "as noted", "as discussed" and any dialogue with an imagined critic or reviewer should not appear. A document may describe itself in two places only: a dateline under the title, giving the date and any companion documents, and a method section or appendix where one exists.

In documents meant to be read as prose, do not break into bullet points or numbered lists. Where several items must be enumerated, carry them inside a sentence with commas or semicolons. Lists belong in reference material (a rulebook such as this one, a README index, a requirements table) and wherever the reader has asked for one.

## 2. Words and phrases to avoid

Each row names a kind of fault, says why it reads as generated, lists the terms to search for, and says what to do instead. The lists are not exhaustive; use the reason column to judge a term that is not listed.

| Kind of fault | Why it fails | Search for | Do instead |
|---|---|---|---|
| Commenting on the utterance | Telling the reader that a statement is honest, or writing "honestly", implies the writer is lying the rest of the time. Announcing that a point is difficult, real or worth saying delays the point and asks for credit for making it. | worth noting, worth saying, worth knowing, is worth, are worth, the honest answer is, honestly, honest, candid, candidly, I'll be direct, to be clear, let me be clear, the real question, there's something real here, a real, genuinely, the harder truth is, the uncomfortable bit, uncomfortable, straightforward, that costs something, it bears repeating, make no mistake | Delete the announcement and keep the statement. |
| Lecturing the reader | Tells the reader what to think of the point instead of making it. | the reader should, crucially, importantly, notably, it should be noted, it is important to, one might, rhetorical questions in the body, praise of the document's own method | State the point and let its placement carry the emphasis. |
| Throat-clearing | Restates the task or signals care before doing anything. | great question, let me think about this, I want to be careful here, before answering, any opening that repeats the question back | Open on the substance. |
| Managing the reader's feelings | Names what the reader presumably feels, validates before answering, or asks whether the reply helped. | I understand this is frustrating, that makes sense, does this help, hope this helps | Answer. |
| Filler and intensifiers | Adds emphasis without information; the sentence is stronger without them. | actually, really, simply, just, very, quite, genuinely, arguably, clearly, obviously, of course, in fact, indeed | Delete. |
| Jargon | A term of art that only a specialist reads without effort excludes every other reader and often covers an imprecise thought. This applies to the writer's own field's vocabulary as much as anyone else's. | any acronym undefined on first use; terms from one discipline given to a general reader without a gloss (idempotent, eventual consistency, pharmacokinetic, heteroskedastic, amortisation, tort); product and framework names used as if the reader knows them | Use the lay term. Where the technical term is needed, give it once with a plain explanation beside it and use it consistently afterwards. |
| Rarefied or Latinate wording | A long or learned word where a short one exists reads as display. Orwell: never use a long word where a short one will do. | synthesised, instructive, salient, orthogonal (outside mathematics), non-trivial, epistemic, adjudicate, canonical (outside its technical sense), modulo, granular, leverage (verb), operationalise, surface (verb), unpack, interrogate (of data), problematise, de-risk, robust (as praise), compelling, powerful, novel, elegant, utilise, facilitate, commence, prior to, in order to, the aforementioned, the whole of, nothing less than, precisely (as intensifier), in its own terms, properly understood, at bottom, writ large | Use the plain word: made, useful, notable, unrelated, hard, judge, standard, fine-grained, use, run, raise, explain, examine, before, to. |
| Figurative language from another field | A metaphor borrowed from building, finance, warfare, sport or theatre makes the reader translate it back. "Load-bearing" describes a wall or a beam; used of a claim or a datum it is a picture where a fact was wanted. Technical prose names things in their own vocabulary. | load-bearing (outside a physical structure), headline (outside a headline), substrate, buys, bet, sits, lives, lands, surfaces, bites, erodes, collapses, carries weight, carries the argument, does the work, earns a place, pulls, pushes back, eats, wedges, window (as opportunity), trap, moat, north star, table stakes, plumbing, backbone, spine, engine (outside physics or software), machinery (of a process), lens, frame, framing, story, narrative, the X conversation, in the room, on the shelf, moves the needle, bandwidth, signal (non-technical), vocabulary or grammar (of a market), landscape (outside a title), ecosystem (outside biology), space (as a market), plays to, wins, loses, kills, survives, stands up, holds, carries (of a claim), cost, pays for (of non-money), cheap, expensive, sharp, clean, elegant, rich, thin, deep (of non-physical things), rests on, sits underneath, pull in different directions | Ask whether the phrase would be literal in some other field. If so, name the thing directly: a datum is relied on; an argument is supported by the evidence. |
| Dramatic contrast | "X does this; it does not do that" and "not A but B" set up an opposition the reader did not need, to make a plain statement sound decisive. | X, not Y; not X but Y; it is not about X, it is about Y; X does..., it does not...; X rather than Y (where Y was never in question) | State what is the case. If the negative matters, give it its own sentence with a reason. |
| Signposting | A sentence that announces how many points follow, or that a point is about to be made, delays the point and reads as a lecture. | there are three reasons, the gap is in three places, two things follow, the answer has two parts, this has several consequences, first ... second ... third | Make the points. If the reader needs to know they are related, the paragraph's shape will show it. |
| Personifying the text | A sentence, paragraph or document does not want, argue, insist or tell; the writer does. Giving the text intentions presents an editorial judgement as an observation. | the sentence wants, the paragraph argues, the document insists, the data tells us, the evidence speaks, this section makes the case | Say who is doing the thing, or state the judgement directly: split the sentence; the evidence shows. |
| Model idiom | Terms from model evaluation and prompt writing that have leaked into general prose. | the claim (for a paragraph's point), nuanced, delve, it's important to note, in today's fast-paced world, at the end of the day | Use the ordinary word: the point, the position. |

## 3. Mechanics

Do not use em dashes, in documents, replies and commit messages alike. Choose each replacement to suit its sentence: a colon where the dash introduces what follows, a comma pair or parentheses around a parenthetical remark, a semicolon between two independent clauses, a comma for an appositive. Having replaced them, read the changed sentences again, because the substitution introduces its own faults: comma splices, colons and semicolons used where they do not belong, parentheses that now enclose the wrong words, and proper nouns left lower-cased after a colon.

Use semicolons sparingly, and do not let them absorb the work the em dashes were doing. Where a sentence contains more than one, consider splitting it.

Write in British English: recognise, organisation, programme, licence as a noun, defence, metre, analyse, centre.

Prefer understatement. Let the evidence come before the conclusion, and do not open a document or a section with its largest figure or its strongest statement.

Write in full sentences, each with a subject and a finite verb. Avoid clipped technical shorthand in documentation and replies, and expand an abbreviation the first time it appears.

## 4. Evidence and sources

When research finds no example of something, say that nothing was found in the sources searched, and say what was searched. This is weaker than saying the thing does not exist, and a conclusion that depends on the difference should state upon which of the two it relies.

Do not use emoji anywhere in a document. Where a figure is self-reported, vendor-sourced or otherwise unverified, say so in words in the sentence that gives it, naming who reported it: "The vendor reported two million users in its launch announcement; no independent count was found." Footnotes are easy to skip; the caveat belongs beside the number.

Every reference to an external source carries a link to it, at the point in the text where the source is used. Every quotation is attributed to its author and source, with the link, in the sentence that contains it or immediately after. Section references into a companion document are written in square brackets, for example `[§6.6]`, and links between documents are relative links, so that a reader can follow a reference without a description of which document says what.

Every document that draws on research ends with a bibliography listing every source consulted, whether or not it is cited in the body, with title, publisher or author, date and link. Where only trade press or vendor material was available on a point, say so in the sentence that uses it. Prefer a programme's own documents, a standard's own text or a paper over trade press and vendor material.

Hold the writer's own organisation's figures to the standard applied to everyone else's: give the comparator, the exact figure, and what is wrong with any number in wide circulation, in the same words that would be used for a competitor.

## 5. The check before a document is finished

Work through these passes in order on any document before it is pushed, and on any reply longer than a paragraph. Each pass reads the whole draft for one thing only. Make the corrections after each pass, then re-read the changed passages before the next.

1. Read the draft once straight through as its intended reader, without stopping to edit, and note where attention wandered or a sentence had to be read twice. Those places are where the later passes should look hardest.
2. Read the headings alone, in sequence, with no body text. Rewrite any that ask a question, assert a conclusion, contain a count, or would fit any other document, as a short noun phrase naming the section's subject.
3. Read the first sentence of every paragraph in turn. Where three or more in a row share a shape, or open with a bolded label, vary them and fold the label into the paragraph.
4. Read the last sentence of every paragraph and section. Where it is a summing-up, an epigram or a slogan, delete it or move the point into the body.
5. Read for the document describing itself outside the dateline and any method section; for sentences that announce the points to come; for sentences that set up a contrast to sound decisive; and for sentences that give the text intentions of its own. Rewrite each as a plain statement of what is the case.
6. Take the table in §2 one row at a time and read the draft for that kind of fault, using the reason column to catch terms not listed. Leave any term used in its literal sense.
7. Read for jargon and abbreviations. Every technical term is either a lay term or explained on first use; every abbreviation is expanded on first use.
8. Search the draft for `—` and `--`. Replace each as §3 describes, then re-read every sentence changed, since the replacement is where comma splices and misplaced colons appear.
9. Read for the evidence conventions in §4: every finding of absence names what was searched, every unverified figure names who reported it, every external reference and quotation carries a link and an attribution, no emoji appear, spellings are British, and the bibliography lists every source consulted.
10. Read the draft once more straight through. If a passage still reads as generated and no rule above names why, describe the fault in a sentence and add it to this skill.

## 6. Examples

Each pair shows a passage as first written and as corrected. The fault is named once; the correction is left to show its own reasoning.

A heading that asks a question. Before: "## 1. Who loses when a delivery is late?" After: "## 1. Customers affected by late deliveries". The section's subject is those customers; the heading names them.

A paragraph that describes the document. Before: "This document collects the questions the earlier notes did not ask. None is answered here; each is stated with the reason it matters." After: "The earlier notes each asked whether a technical gap exists, and each found one. The questions that remain concern demand, timing and price." The second version says what the earlier work established and what is still open, and leaves the reader to see that the questions follow.

A closing epigram. Before: "Neither is urgent for a demonstration; both are urgent for a contract." After: "Neither matters for a demonstration. Both must be settled before a contract is signed, because the buyer's lawyers will ask about them." The correction gives the reason the original implied and drops the balanced shape.

A dramatic contrast. Before: "Funding is a fixed-price contract, not a grant." After: "Funding is provided under a fixed-price contract, with the reporting and delivery obligations a contract carries." The reader learns what a contract means here, which was the point.

A signpost. Before: "The problem is in three places. First, most bus stops have no shelter. Second, timetables are printed in English only. Third, no route runs after midnight." After: "Most of the city's bus stops have no shelter, and the few that do are in the centre. Timetables are printed in English only, although many riders read another language more easily. No route runs after midnight; the last service in every timetable examined leaves before 23:45."

Personified text. Before: "A sentence with three semicolons wanted to be two sentences." After: "Where a sentence contains more than one semicolon, consider splitting it."

Figurative language. Before: "The risk is one of posture: the other suppliers may read the comparison as an attack." After: "The risk is in how the comparison is received: the other suppliers may read it as an attack."

Uniform paragraph shape. Before, three consecutive paragraphs each opening with a bold label and a single justifying sentence: "**Shelter.** Most bus stops have no shelter. **Language.** Timetables are printed in English only. **Late services.** No route runs after midnight." After: the single paragraph given under the signpost example above, in which the same content is written as ordinary sentences of varying length.

Jargon without a gloss. Before: "After the ACE inhibitor was started her eGFR fell below 30, so the GP ordered a repeat U&E." After: "After she began taking an ACE inhibitor, a type of blood-pressure drug, a blood test showed that her kidneys were filtering less than a third of the normal amount, so her family doctor ordered a second test of the salts and waste products in her blood."
