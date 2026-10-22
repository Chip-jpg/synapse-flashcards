"""Makes the Anki packages `anki.rs` tests read, with Anki itself.

Run with the `anki` package from PyPI (25.09 made the files here):

    python3 -m venv venv && venv/bin/pip install anki
    venv/bin/python make_fixtures.py OUTPUT_FOLDER

It writes `new.apkg` (the current format: zstd-compressed collection.anki21b
beside a "please update Anki" collection.anki2), `legacy.apkg` (with "support
older Anki versions": collection.anki21), and `all.colpkg` (a whole
collection). Copy those three here. No media is included.

It also writes `occlusion.apkg` and `occlusion-legacy.apkg` from a second
collection, with media, for image occlusion, and `pictures.apkg` and
`pictures-legacy.apkg` from a third, with media, for pictures on front/back
cards, and `tags.apkg` and `tags-legacy.apkg` from a fourth, for tags (see
the end of this file).

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
import struct
import sys
import zlib

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


# Image occlusion, with its pictures: a second collection, exported with
# media, in the newer and the legacy format.
#   Anatomy: "Heart" (a rectangle hiding all, an ellipse and a polygon sharing
#   c2, a text label c3, and a Back Extra); a note on the same picture with
#   two rectangles numbered c1 and c3 that hides one; a note whose picture is
#   missing; one with pixel coordinates (early Anki); and a Basic card.
def png(width, height):
    rows = b"".join(b"\x00" + b"\xff\x00\x00" * width for _ in range(height))

    def chunk(kind, data):
        crc = zlib.crc32(kind + data) & 0xFFFFFFFF
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", crc)

    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(rows))
        + chunk(b"IEND", b"")
    )


os.makedirs(os.path.join(out, "work2"))
col = Collection(os.path.join(out, "work2", "collection.anki2"))
models = col.models
anatomy = col.decks.id("Anatomy")
occlusion = models.by_name("Image Occlusion")
heart = os.path.join(out, "work2", "heart.png")
with open(heart, "wb") as file:
    file.write(png(200, 100))


def add_occlusion(occlusions, header, back_extra=""):
    col.add_image_occlusion_note(
        notetype_id=occlusion["id"],
        image_path=heart,
        occlusions=occlusions,
        header=header,
        back_extra=back_extra,
        tags=[],
    )
    note = col.get_note(max(col.find_notes('"note:Image Occlusion"')))
    col.set_deck([card.id for card in note.cards()], anatomy)
    return note


add_occlusion(
    "{{c1::image-occlusion:rect:left=.1:top=.2:width=.3:height=.4:oi=1}}"
    "{{c2::image-occlusion:ellipse:left=.5:top=.1:rx=.1:ry=.05:oi=1}}"
    "{{c2::image-occlusion:polygon:points=.6,.6 .8,.7 .7,.9:oi=1}}"
    "{{c3::image-occlusion:text:left=.1:top=.8:text=Aorta:scale=1:fs=.05}}",
    "Heart",
    "Some extra",
)
add_occlusion(
    "{{c1::image-occlusion:rect:left=0:top=0:width=.5:height=.5}}"
    "{{c3::image-occlusion:rect:left=.5:top=.5:width=.5:height=.5}}",
    "",
)
gone = add_occlusion("{{c1::image-occlusion:rect:left=.1:top=.1:width=.2:height=.2:oi=1}}", "Gone")
gone["Image"] = '<img src="gone.png">'
col.update_note(gone)
add_occlusion("{{c1::image-occlusion:rect:left=20:top=10:width=60:height=30:oi=1}}", "Pixels")
note = col.new_note(models.by_name("Basic"))
note["Front"], note["Back"] = "Pump", "Heart"
col.add_note(note, anatomy)

options = ExportAnkiPackageOptions(
    with_scheduling=False, with_deck_configs=False, with_media=True, legacy=False
)
col.export_anki_package(out_path=os.path.join(out, "occlusion.apkg"), options=options, limit=None)
options.legacy = True
col.export_anki_package(
    out_path=os.path.join(out, "occlusion-legacy.apkg"), options=options, limit=None
)
col.close()


# Pictures on front/back cards: a third collection, exported with media, in
# the newer and the legacy format.
#   Pictures: "Heart" (a PNG on the front, a GIF on the back); "Valve", a
#   reversed note showing the same PNG twice on its front; "Many" (five
#   pictures on the front, one past the four a side shows); "Gone" (a picture
#   missing from the package, and one by web address); "Vector" (an SVG);
#   "Sound" (a picture and a sound); a cloze note with a picture; and a note
#   whose front is only a picture (empty).
os.makedirs(os.path.join(out, "work3"))
col = Collection(os.path.join(out, "work3", "collection.anki2"))
models = col.models
pictures = col.decks.id("Pictures")
GIF = bytes.fromhex("47494638396101000100800000000000ffffff21f90401000000002c00000000010001000002024401003b")
media = {
    "heart.png": png(4, 2),
    "dot.gif": GIF,
    "shape.svg": b'<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>',
}
for number in range(1, 6):
    media[f"p{number}.png"] = png(number, 1)
for name, data in media.items():
    assert col.media.write_data(name, data) == name

add("Basic", pictures, 'Heart <img src="heart.png">', 'Pumps blood <img src="dot.gif">')
add("Basic (and reversed card)", pictures, 'Valve <img src="heart.png"><img src="heart.png">', "Lets blood one way")
add("Basic", pictures, "Many " + "".join(f'<img src="p{n}.png">' for n in range(1, 6)), "Five pictures")
add("Basic", pictures, 'Gone <img src="gone.png">', 'Web <img src="https://example.com/a.png">')
add("Basic", pictures, 'Vector <img src="shape.svg">', "An SVG")
add("Basic", pictures, 'Sound <img src="heart.png">', "Beat [sound:beat.mp3]")
add("Cloze", pictures, '{{c1::Heart}} <img src="heart.png">', "")
add("Basic", pictures, '<img src="heart.png">', "Only a picture")

options = ExportAnkiPackageOptions(
    with_scheduling=False, with_deck_configs=False, with_media=True, legacy=False
)
col.export_anki_package(out_path=os.path.join(out, "pictures.apkg"), options=options, limit=None)
options.legacy = True
col.export_anki_package(
    out_path=os.path.join(out, "pictures-legacy.apkg"), options=options, limit=None
)
col.close()


# Tags: a fourth collection, in the newer and the legacy format.
#   Tagged: "Cell" (bio::cells, Exam, and a 61-character tag Synapse can't
#   take); "Chat", a reversed note tagged french; a cloze note tagged bio;
#   "Many" with 22 tags (two past a card's 20); "Plain" with none; and an
#   image occlusion note tagged anatomy.
os.makedirs(os.path.join(out, "work4"))
col = Collection(os.path.join(out, "work4", "collection.anki2"))
models = col.models
tagged = col.decks.id("Tagged")


def add_tagged(note_type, tags, *fields):
    note = add(note_type, tagged, *fields)
    note.tags = tags
    col.update_note(note)


add_tagged("Basic", ["bio::cells", "Exam", "x" * 61], "Cell", "Unit of life")
add_tagged("Basic (and reversed card)", ["french"], "chat", "cat")
add_tagged("Cloze", ["bio"], "{{c1::Mitosis}} divides {{c2::cells}}", "")
add_tagged("Basic", [f"t{number:02}" for number in range(1, 23)], "Many", "Tags")
add_tagged("Basic", [], "Plain", "No tags")
picture = os.path.join(out, "work4", "cell.png")
with open(picture, "wb") as file:
    file.write(png(4, 4))
col.add_image_occlusion_note(
    notetype_id=models.by_name("Image Occlusion")["id"],
    image_path=picture,
    occlusions="{{c1::image-occlusion:rect:left=.1:top=.1:width=.3:height=.3:oi=1}}",
    header="Cell",
    back_extra="",
    tags=["anatomy"],
)
io_note = col.get_note(col.find_notes('"note:Image Occlusion"')[0])
col.set_deck([card.id for card in io_note.cards()], tagged)

options = ExportAnkiPackageOptions(
    with_scheduling=False, with_deck_configs=False, with_media=True, legacy=False
)
col.export_anki_package(out_path=os.path.join(out, "tags.apkg"), options=options, limit=None)
options.legacy = True
col.export_anki_package(out_path=os.path.join(out, "tags-legacy.apkg"), options=options, limit=None)
col.close()
