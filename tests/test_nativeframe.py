from calltx.ui import nativeframe as nf


def test_keep_frame_style_restores_bits():
    assert nf.keep_frame_style(0x16000000) & nf.FRAME_BITS == nf.FRAME_BITS
    assert nf.keep_frame_style(0x16000000) & 0x16000000 == 0x16000000


def test_frame_inset_uses_dpi():
    seen = []

    def metric(i, dpi):
        seen.append((i, dpi))
        return {32: 4, 33: 4, 92: 4}[i] * dpi // 96

    assert nf.frame_inset(144, metric) == (12, 12)
    assert all(d == 144 for _, d in seen)


def test_client_rect_normal_and_maximized():
    assert nf.client_rect((0, 0, 1000, 700), False, (8, 8)) == (0, 0, 1000, 700)
    assert nf.client_rect((-8, -8, 2568, 1400), True, (8, 8)) == (0, 0, 2560, 1392)


def test_hit_test_zones():
    w, h, b = 1000, 700, 5
    assert nf.hit_test(500, 300, w, h, b, False) == 1
    assert nf.hit_test(2, 300, w, h, b, False) == 10
    assert nf.hit_test(998, 300, w, h, b, False) == 11
    assert nf.hit_test(500, 1, w, h, b, False) == 12
    assert nf.hit_test(500, 698, w, h, b, False) == 15
    assert nf.hit_test(3, 3, w, h, b, False) == 13
    assert nf.hit_test(997, 697, w, h, b, False) == 17
    assert nf.hit_test(2, 300, w, h, b, True) == 1
