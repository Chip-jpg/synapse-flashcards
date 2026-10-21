Synapse can now read the text in pictures and scanned pages. A text-recognition engine is built into the app and works offline. For accented letters and handwriting, you can also choose a vision model you run yourself in Ollama. Either way, recognised text is always shown as recognised, for you to check, and nothing leaves your computer. **This release adds a migration, so backups made with it can't be restored into 0.8.1 or older** (see *Backups* below).

## What's new

**Read scanned PDFs.**

- Until now, a PDF made of scanned pages had no text, so it couldn't be imported at all.
- Now *Import file* keeps such a PDF and offers three choices: *Read scanned pages*, *Write the note myself*, or *Cancel*. After the pages are read, the new-note form opens with their text.
- On the *Sources* screen, a PDF with pages that have no text offers *Read scanned pages (N)*. Reading shows "Reading page 3 of 12…" with a progress bar and *Stop*. Leaving the screen also stops it, and pages already read are kept.
- At the end you get a summary: which pages were read, which had nothing to find, and which were skipped, each with a reason in plain words.
- Each page of a source shows what was recognised on it, set apart under "Recognised text, check for mistakes". It says which engine read the page and when. *Read this page again* reads just that one page.
- *Make a note* from a source uses the recognised text where a page had none of its own.
- Links like "From page 7 of …" on suggested cards and quiz questions now work on recognised pages too.

**Text in pictures.**

- Every picture in a note, and in the note and card forms, has *Text in picture*. It shows the text found in a panel marked as recognised, with *Copy*.
- In a form you also get *Add to note* (or *Add to front* / *Add to back*). Nothing is saved until you save the form.
- *Notes* → *Note from a photo*: choose a picture, and a new note opens with its text and the picture attached. You check it and give it a title.
- On a picture card (image occlusion), *Find labels* proposes a box over each line of text, or over each word. Each box is labelled with that text and becomes its own card. You can check, move or remove the boxes before saving. Labels side by side on a diagram are split into separate boxes. Sometimes a stray letter is read from a drawn shape, and you can just remove it.

**Choose who reads: built in, or a local model.**

- The built-in engine is fast, about half a second per page, and needs nothing installed. It only reads letters without accents, though: Italian "è" comes out as "e", and some accented letters come out as "?". It also reads handwriting poorly.
- Everywhere text is read, *Read with* lets you pick *Built in* or a vision model you've installed in Ollama on this computer, such as qwen2.5vl or llama3.2-vision. Synapse remembers your choice.
- These models read accents and handwriting much better. In a test on an Italian scanned page, every accent came out right. They are much slower, though: a few minutes per page without a graphics card.
- The same rules as suggested cards apply. Only Ollama on this computer is asked, cloud models are never listed, and nothing is sent until you press the button.
- A model can make up text, so Synapse cleans its answer and refuses an empty one. When the built-in engine has read the same picture, Synapse also shows how many of its words the model found too (for example "The model's text has 96% of the words the built-in reader found"), as a hint.
- *Find labels* always uses the built-in engine, because only it says where on the picture the text is.

## Backups

**This release adds migration 0019** (`recognitions`, the text read from each picture and page).

- Backups made with this version restore into this version and newer, but **0.8.1 and older refuse them** as "from a newer version".
- Backups made with 0.8.1 and older still restore into this version.
- The backup format stays 2, and recognised text travels inside the backup.

## Bigger download

The built-in engine's two models are part of the program, so nothing is ever downloaded. As a result, the installers are about **11 MB larger** than 0.8.1.

## Not tried yet

- Everything was checked by automated tests: 310 for the screens, plus 386 for the Rust side and 10 that run the real helper process. It was also tried in the real app on Linux, with a scratch database.
- Nothing has been tried on Windows yet.
- The built-in engine hasn't been tried on a wide range of real scans. JPEG 2000 and JBIG2 scans (some scanners use them) are reported as unreadable.
- Recognised text isn't searched by *Search* yet.

Windows x64 installers:

- Synapse_0.9.0_x64_en-US.msi
- Synapse_0.9.0_x64-setup.exe

The installers are unsigned, so Windows SmartScreen may warn on first run. They were built on GitHub's Windows machines by the Release workflow, after its tests passed.
