from calltx.platforms import platform_for_key, site_from_title


def test_platform_for_registry_keys():
    assert platform_for_key("MSTeams_8wekyb3d8bbwe").key == "teams"
    assert platform_for_key("5319275A.WhatsAppDesktop_cv1g1gvanyjgm").key == "whatsapp"
    assert platform_for_key("NonPackaged/C:#Users#x#AppData#Roaming#Zoom#bin#Zoom.exe").key == "zoom"
    assert platform_for_key("NonPackaged/C:#Program Files#BraveSoftware#Brave-Browser#Application#brave.exe").name == "Brave"
    assert platform_for_key("NonPackaged/C:#x#Discord#app-1.0.9#Discord.exe").setting == "detect_discord"
    assert platform_for_key("NonPackaged/C:#Program Files#Mozilla Firefox#firefox.exe").setting == "detect_browsers"
    for other in ("NonPackaged/C:#x#dictado.exe", "NonPackaged/C:#x#Transcriptor.exe",
                  "NonPackaged/C:#x#python.exe", "NonPackaged/C:#x#game.exe",
                  "Microsoft.WindowsSoundRecorder_8wekyb3d8bbwe"):
        assert platform_for_key(other) is None


def test_site_from_title():
    assert site_from_title("Meet - abc-defg-hij - Google Chrome") == "Meet"
    assert site_from_title("Google Meet — Mozilla Firefox") == "Meet"
    assert site_from_title("Reunión | Microsoft Teams — Mozilla Firefox") == "Teams"
    assert site_from_title("Zoom Meeting - Brave") == "Zoom"
    assert site_from_title("(3) WhatsApp - Google Chrome") == "WhatsApp"
    assert site_from_title("YouTube - Google Chrome") is None
    assert site_from_title("Meeting notes – Google Docs — Mozilla Firefox") is None
    assert site_from_title("Meetup: Python Madrid — Mozilla Firefox") is None
