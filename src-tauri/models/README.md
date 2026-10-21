# Text recognition models

The two neural network models the built-in text recognition engine uses (`src/ocr.rs`). They are
compiled into Synapse with `include_bytes!`, so nothing is downloaded when the app runs.

| File | What it does | SHA-256 |
| :-- | :-- | :-- |
| `text-detection.rten` | Finds words on a picture | `f15cfb56bd02c4bf478a20343986504a1f01e1665c2b3a0ad66340f054b1b5ca` |
| `text-recognition.rten` | Reads a line of text | `e484866d4cce403175bd8d00b128feb08ab42e208de30e42cd9889d8f1735a6e` |

A test (`the_bundled_models_are_the_ones_named` in `ocr.rs`) checks both hashes. A new model needs
a new `ocr::MODEL` name too, so results from the old and new models can be told apart.

## Where they come from

- Engine: [ocrs](https://github.com/robertknight/ocrs) by Robert Knight (MIT or Apache-2.0).
- Models: the pre-trained ocrs models,
  [huggingface.co/robertknight/ocrs](https://huggingface.co/robertknight/ocrs) (`text-detection-ssfbcj81`
  and `text-rec-checkpoint-s52qdbqt`), downloaded unchanged from
  `https://ocrs-models.s3-accelerate.amazonaws.com/text-detection.rten` and `…/text-recognition.rten`.
  They were trained on [HierText](https://github.com/google-research-datasets/hiertext) and
  synthetic data.

## Licence

The models are © Robert Knight, licensed under
[Creative Commons Attribution-ShareAlike 4.0](https://creativecommons.org/licenses/by-sa/4.0/)
(CC BY-SA 4.0). They are included unchanged. That licence covers these two files only; the rest of
Synapse stays under its own MIT licence.

## What they can read

The Latin alphabet without accents, digits and ASCII punctuation: "è" comes out as "e", and some
accented letters as "?". Printed text reads well; handwriting poorly.
