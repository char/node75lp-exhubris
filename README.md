# node75lp-exhubris

keyboard firmware in Rust for the NuPhy Node75 LP (WCH CH582) :D

`cargo xtask fetch` grabs hubris and exhubris and patches them and puts them under `vendor/`

## building

```shell
cargo xtask build  # firmware image in target/node75/
cargo xtask test   # build, then run tests against the built firmware
```

## flashing

first, get the keyboard in loader mode.
if running stock firmware, you can do this via `uv run tools/upload.py enter-loader --path <hid path>`;
if running this firmware, you can plug the keyboard in while holding down the space bar.

```shell
$ # locate the loader's HID path:
$ uv run tools/upload.py list
$ # flash the image
$ uv run tools/upload.py flash target/node75/node75.experimental.bin --path '<loader path from list>'
```
