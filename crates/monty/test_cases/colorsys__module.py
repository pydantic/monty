# colorsys is vendored from CPython's own source; see crates/monty/stdlib/
import colorsys
import colorsys as again
from colorsys import ONE_SIXTH, ONE_THIRD, TWO_THIRD, hls_to_rgb, hsv_to_rgb, rgb_to_hls, rgb_to_hsv

# === Module identity ===
# a vendored module is run once per session and cached
assert colorsys is again
assert colorsys.__name__ == 'colorsys'
assert colorsys.__all__ == ['rgb_to_yiq', 'yiq_to_rgb', 'rgb_to_hls', 'hls_to_rgb', 'rgb_to_hsv', 'hsv_to_rgb']
assert (ONE_THIRD, ONE_SIXTH, TWO_THIRD) == (1.0 / 3.0, 1.0 / 6.0, 2.0 / 3.0)

# === YIQ ===
assert colorsys.rgb_to_yiq(1.0, 1.0, 1.0) == (0.9999999999999999, 5.2180482157382354e-17, 9.880984919163892e-17)
assert colorsys.rgb_to_yiq(0.1, 0.2, 0.3) == (0.181, -0.09206999999999999, 0.009910000000000002)
assert colorsys.yiq_to_rgb(0.5, 0.9, 0.9) == (1.0, 0.0, 1.0)
assert colorsys.yiq_to_rgb(0.5, -0.9, -0.9) == (0.0, 1.0, 0.0)

# === HLS ===
assert rgb_to_hls(0.2, 0.4, 0.4) == (0.5, 0.30000000000000004, 0.3333333333333333)
assert rgb_to_hls(0.5, 0.5, 0.5) == (0.0, 0.5, 0.0)
assert hls_to_rgb(0.5, 0.5, 0.5) == (0.25, 0.7499999999999999, 0.75)
assert hls_to_rgb(0.0, 0.3, 0.0) == (0.3, 0.3, 0.3)

# === HSV ===
assert rgb_to_hsv(0.2, 0.4, 0.4) == (0.5, 0.5, 0.4)
assert rgb_to_hsv(0.0, 0.0, 0.0) == (0.0, 0.0, 0.0)
assert [hsv_to_rgb(h / 6, 1.0, 1.0) for h in range(6)] == [
    (1.0, 0.0, 0.0),
    (1.0, 1.0, 0.0),
    (0.0, 1.0, 0.0),
    (0.0, 1.0, 1.0),
    (0.0, 0.0, 1.0),
    (1.0, 0.0, 1.0),
]
assert hsv_to_rgb(0.3, 0.0, 0.7) == (0.7, 0.7, 0.7)

# === Errors come from the vendored Python code ===
try:
    hsv_to_rgb('a', 1.0, 1.0)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "can't multiply sequence by non-int of type 'float'"

# === Not a package ===
try:
    import colorsys.missing

    assert False, 'expected ModuleNotFoundError'
except ModuleNotFoundError as exc:
    assert str(exc) == "No module named 'colorsys.missing'; 'colorsys' is not a package"
