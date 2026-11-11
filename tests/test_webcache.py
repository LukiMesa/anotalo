def test_clear_web_cache_removes_http_caches_only(tmp_path):
    from calltx.ui.window import clear_web_cache

    default = tmp_path / "EBWebView" / "Default"
    for d in ("Cache", "Code Cache", "Local Storage", "Session Storage"):
        (default / d / "sub").mkdir(parents=True)
        (default / d / "sub" / "f").write_text("x", encoding="utf-8")
    clear_web_cache(tmp_path)
    assert not (default / "Cache").exists() and not (default / "Code Cache").exists()
    assert (default / "Local Storage" / "sub" / "f").exists() and (default / "Session Storage").exists()
    clear_web_cache(tmp_path / "no-existe")  # sin perfil todavía: no falla
