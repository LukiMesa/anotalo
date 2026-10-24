from calltx import paths


def test_data_dir_frozen_vs_dev():
    assert paths.data_dir(frozen=True).name == "CallTranscriber"
    assert paths.data_dir(frozen=False).name == "CallTranscriber-dev"


def test_default_output_dir_dev_suffix():
    assert paths.default_output_dir(frozen=True).name == "Llamadas"
    assert paths.default_output_dir(frozen=False).name == "Llamadas (dev)"


def test_instance_names_differ():
    assert paths.instance_names(frozen=True) != paths.instance_names(frozen=False)
    assert paths.instance_names(frozen=True)[0] == "CallTranscriberMutex"


def test_web_dir_has_index():
    assert (paths.web_dir() / "index.html").is_file()
    assert (paths.web_dir() / "fonts" / "JetBrainsMono.ttf").is_file()


def test_cuda_dir_in_localappdata():
    assert paths.cuda_dir().parts[-2:] == ("Transcriptor", "cuda")


def test_dev_and_installed_serve_the_page_on_different_ports():
    # pywebview sirve la página en un puerto local fijo: si coinciden, la copia de desarrollo
    # carga los archivos de la instalada (o al revés).
    from calltx.paths import http_port

    assert http_port(frozen=True) != http_port(frozen=False)
    assert all(1024 < http_port(frozen=f) < 65536 for f in (True, False))
