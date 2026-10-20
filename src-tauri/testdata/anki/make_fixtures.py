"""Makes the Anki packages `anki.rs` tests read, with Anki itself.

Run with the `anki` package from PyPI (25.09 made the files here):

    python3 -m venv venv && venv/bin/pip install anki
    venv/bin/python make_fixtures.py OUTPUT_FOLDER

It writes `new.apkg` (the current format: zstd-compressed collection.anki21b
beside a "please update Anki" collection.anki2), `legacy.apkg` (with "support
older Anki versions": collection.anki21), and `all.colpkg` (a whole
collection). Copy those three here. No media is included.

What the collection holds, as `anki.rs`'s tests expect:
- Biology: a Basic card with HTML, a Basic card with a 2,001-character back
  (too long), a cloze note with one deletion, one with two numbers (one used
  twice, one with a hint), HTML and a "Back Extra", one with nested
  deletions (which Synapse doesn't accept), and an image occlusion note.
- Biology::Cells: a Basic card with entities and divs, one with an image and a
  sound beside text, and one whose front is only an image (empty).
- French: "Basic (and reversed card)", "Basic (optional reversed card)" with
  and without the reverse, "Basic (type in the answer)", and a custom
  three-field note type (another type). The "chat" card has been reviewed.
"""

import os
import sys

from anki.collection import Collection, ExportAnkiPackageOptions

out = sys.argv[1]
os.makedirs(os.path.join(out, "work"))
col = Collection(os.path.join(out, "work", "collection.anki2"))
models = col.models


def add(note_type, deck, *fields):
    note = col.new_note(models.by_name(note_type))
    for index, value in enumerate(fields):
        note.fields[index] = value
    col.add_note(note, deck)
    return note


biology = col.decks.id("Biology")
cells = col.decks.id("Biology::Cells")
french = col.decks.id("French")

add("Basic", biology, "What is a <b>cell</b>?", "The basic unit of life.<br>Found in all organisms.")
add("Basic", cells, "Mitochondria&nbsp;role", "<div>Make energy</div><div>(ATP) &amp; more &lt;3</div>")
add("Basic", cells, 'Picture <img src="cell.png">', "A cell [sound:cell.mp3]")
add("Basic", cells, '<img src="only.png">', "Only an image on the front")
add("Basic", biology, "Long", "x" * 2001)
add("Basic (and reversed card)", french, "chat", "cat")
add("Basic (optional reversed card)", french, "chien", "dog", "y")
add("Basic (optional reversed card)", french, "oiseau", "bird", "")
add("Basic (type in the answer)", french, "maison", "house")
add("Cloze", biology, "{{c1::Mitosis}} divides cells", "")
add(
    "Cloze",
    biology,
    "{{c1::Ribosomes}} make {{c2::proteins::what?}}<br>in the <b>{{c1::cytoplasm}}</b>",
    "Also on the <i>rough ER</i>",
)
add("Cloze", biology, "{{c1::Outer {{c2::inner}} text}}", "")

# Image occlusion needs its picture: a 1x1 PNG, which isn't exported.
picture = os.path.join(out, "work", "cell.png")
with open(picture, "wb") as file:
    file.write(
        bytes.fromhex(
            "89504e470d0a1a0a0000000d4948445200000001000000010806000000"
            "1f15c4890000000d49444154789c6360000002000154a24f5d0000000049454e44ae426082"
        )
    )
occlusion = models.by_name("Image Occlusion")
col.add_image_occlusion_note(
    notetype_id=occlusion["id"],
    image_path=picture,
    occlusions="{{c1::image-occlusion:rect:left=.1:top=.1:width=.3:height=.3:oi=1}}",
    header="Cell",
    back_extra="",
    tags=[],
)
io_note = col.get_note(col.find_notes('"note:Image Occlusion"')[0])
col.set_deck([card.id for card in io_note.cards()], biology)

vocab = models.new("Vocab")
for name in ["Word", "Meaning", "Example"]:
    models.add_field(vocab, models.new_field(name))
template = models.new_template("Card 1")
template["qfmt"] = "{{Word}}"
template["afmt"] = "{{FrontSide}}<hr id=answer>{{Meaning}}<br>{{Example}}"
models.add_template(vocab, template)
models.add(vocab)
add("Vocab", french, "pain", "bread", "du pain")

card = col.get_card(col.find_cards("chat")[0])
card.start_timer()
col.sched.answerCard(card, 3)

options = ExportAnkiPackageOptions(
    with_scheduling=True, with_deck_configs=False, with_media=False, legacy=False
)
col.export_anki_package(out_path=os.path.join(out, "new.apkg"), options=options, limit=None)
options.legacy = True
col.export_anki_package(out_path=os.path.join(out, "legacy.apkg"), options=options, limit=None)
col.export_collection_package(os.path.join(out, "all.colpkg"), include_media=False, legacy=False)
col.close()
