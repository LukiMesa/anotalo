"""Dispositivos y sesiones de audio de Windows (Core Audio), leídos en el momento.

Para cada dispositivo activo, qué programas tienen sonido abierto (y si está sonando o
grabando ahora). Así se sabe en qué auriculares suena la llamada o qué micrófono usa,
y también si una app de llamadas está usando el micrófono aunque el registro de privacidad
de Windows no lo anote.
"""

from __future__ import annotations

import ctypes
import logging
import threading
from ctypes import POINTER, byref, c_int, c_void_p, wintypes
from dataclasses import dataclass, field

import comtypes
from comtypes import COMMETHOD, GUID, HRESULT, IUnknown

from ..platforms import platform_for_key
from .routing import SessionInfo, process_key

log = logging.getLogger(__name__)

E_RENDER, E_CAPTURE = 0, 1
E_CONSOLE, E_COMMUNICATIONS = 0, 2
DEVICE_STATE_ACTIVE = 1
CLSCTX_ALL = 23
AUDIO_SESSION_STATE_ACTIVE = 1
CLSID_MMDEVICE_ENUMERATOR = GUID("{BCDE0395-E52F-467C-8E3D-C4579291692E}")


class PROPERTYKEY(ctypes.Structure):
    _fields_ = [("fmtid", GUID), ("pid", wintypes.DWORD)]


class PROPVARIANT_PTR(ctypes.Structure):
    """PROPVARIANT cuando guarda un puntero (VT_LPWSTR)."""
    _fields_ = [("vt", ctypes.c_ushort), ("r1", ctypes.c_ushort), ("r2", ctypes.c_ushort),
                ("r3", ctypes.c_ushort), ("ptr", c_void_p), ("pad", c_void_p)]


PKEY_DEVICE_FRIENDLY_NAME = PROPERTYKEY(GUID("{a45c254e-df1c-4efd-8020-67d146a850e0}"), 14)


class IPropertyStore(IUnknown):
    _iid_ = GUID("{886d8eeb-8cf2-4446-8d02-cdba1dbdcf99}")
    _methods_ = [
        COMMETHOD([], HRESULT, "GetCount", (["out"], POINTER(wintypes.DWORD), "cProps")),
        COMMETHOD([], HRESULT, "GetAt", (["in"], wintypes.DWORD, "iProp"), (["out"], POINTER(PROPERTYKEY), "pkey")),
        COMMETHOD([], HRESULT, "GetValue", (["in"], POINTER(PROPERTYKEY), "key"),
                  (["in"], POINTER(PROPVARIANT_PTR), "pv")),
    ]


class IMMDevice(IUnknown):
    _iid_ = GUID("{D666063F-1587-4E43-81F1-B948E807363F}")
    _methods_ = [
        COMMETHOD([], HRESULT, "Activate", (["in"], POINTER(GUID), "iid"), (["in"], wintypes.DWORD, "dwClsCtx"),
                  (["in"], c_void_p, "pActivationParams"), (["out"], POINTER(POINTER(IUnknown)), "ppInterface")),
        COMMETHOD([], HRESULT, "OpenPropertyStore", (["in"], wintypes.DWORD, "stgmAccess"),
                  (["out"], POINTER(POINTER(IPropertyStore)), "ppProperties")),
        COMMETHOD([], HRESULT, "GetId", (["out"], POINTER(c_void_p), "ppstrId")),
        COMMETHOD([], HRESULT, "GetState", (["out"], POINTER(wintypes.DWORD), "pdwState")),
    ]


class IMMDeviceCollection(IUnknown):
    _iid_ = GUID("{0BD7A1BE-7A1A-44DB-8397-CC5392387B5E}")
    _methods_ = [
        COMMETHOD([], HRESULT, "GetCount", (["out"], POINTER(wintypes.UINT), "pcDevices")),
        COMMETHOD([], HRESULT, "Item", (["in"], wintypes.UINT, "nDevice"), (["out"], POINTER(POINTER(IMMDevice)), "ppDevice")),
    ]


class IMMDeviceEnumerator(IUnknown):
    _iid_ = GUID("{A95664D2-9614-4F35-A746-DE8DB63617E6}")
    _methods_ = [
        COMMETHOD([], HRESULT, "EnumAudioEndpoints", (["in"], c_int, "dataFlow"), (["in"], wintypes.DWORD, "dwStateMask"),
                  (["out"], POINTER(POINTER(IMMDeviceCollection)), "ppDevices")),
        COMMETHOD([], HRESULT, "GetDefaultAudioEndpoint", (["in"], c_int, "dataFlow"), (["in"], c_int, "role"),
                  (["out"], POINTER(POINTER(IMMDevice)), "ppEndpoint")),
        COMMETHOD([], HRESULT, "GetDevice", (["in"], wintypes.LPCWSTR, "pwstrId"),
                  (["out"], POINTER(POINTER(IMMDevice)), "ppDevice")),
    ]


class IAudioSessionControl(IUnknown):
    _iid_ = GUID("{F4B1A599-7266-4319-A8CA-E70ACB11E8CD}")
    _methods_ = [
        COMMETHOD([], HRESULT, "GetState", (["out"], POINTER(c_int), "pRetVal")),
        COMMETHOD([], HRESULT, "GetDisplayName", (["out"], POINTER(c_void_p), "pRetVal")),
        COMMETHOD([], HRESULT, "SetDisplayName", (["in"], wintypes.LPCWSTR, "Value"), (["in"], POINTER(GUID), "Ctx")),
        COMMETHOD([], HRESULT, "GetIconPath", (["out"], POINTER(c_void_p), "pRetVal")),
        COMMETHOD([], HRESULT, "SetIconPath", (["in"], wintypes.LPCWSTR, "Value"), (["in"], POINTER(GUID), "Ctx")),
        COMMETHOD([], HRESULT, "GetGroupingParam", (["out"], POINTER(GUID), "pRetVal")),
        COMMETHOD([], HRESULT, "SetGroupingParam", (["in"], POINTER(GUID), "Override"), (["in"], POINTER(GUID), "Ctx")),
        COMMETHOD([], HRESULT, "RegisterAudioSessionNotification", (["in"], c_void_p, "NewNotifications")),
        COMMETHOD([], HRESULT, "UnregisterAudioSessionNotification", (["in"], c_void_p, "NewNotifications")),
    ]


class IAudioSessionControl2(IAudioSessionControl):
    _iid_ = GUID("{bfb7ff88-7239-4fc9-8fa2-07c950be9c6d}")
    _methods_ = [
        COMMETHOD([], HRESULT, "GetSessionIdentifier", (["out"], POINTER(c_void_p), "pRetVal")),
        COMMETHOD([], HRESULT, "GetSessionInstanceIdentifier", (["out"], POINTER(c_void_p), "pRetVal")),
        COMMETHOD([], HRESULT, "GetProcessId", (["out"], POINTER(wintypes.DWORD), "pRetVal")),
        COMMETHOD([], HRESULT, "IsSystemSoundsSession"),
    ]


class IAudioSessionEnumerator(IUnknown):
    _iid_ = GUID("{E2F5BB11-0570-40CA-ACDD-3AA01277DEE8}")
    _methods_ = [
        COMMETHOD([], HRESULT, "GetCount", (["out"], POINTER(c_int), "SessionCount")),
        COMMETHOD([], HRESULT, "GetSession", (["in"], c_int, "SessionCount"),
                  (["out"], POINTER(POINTER(IAudioSessionControl)), "Session")),
    ]


class IAudioSessionManager(IUnknown):
    _iid_ = GUID("{BFA971F1-4D5E-40BB-935E-967039BFBEE4}")
    _methods_ = [
        COMMETHOD([], HRESULT, "GetAudioSessionControl", (["in"], POINTER(GUID), "AudioSessionGuid"),
                  (["in"], wintypes.DWORD, "StreamFlags"), (["out"], POINTER(POINTER(IAudioSessionControl)), "SessionControl")),
        COMMETHOD([], HRESULT, "GetSimpleAudioVolume", (["in"], POINTER(GUID), "AudioSessionGuid"),
                  (["in"], wintypes.DWORD, "StreamFlags"), (["out"], POINTER(POINTER(IUnknown)), "AudioVolume")),
    ]


class IAudioSessionManager2(IAudioSessionManager):
    _iid_ = GUID("{77AA99A0-1BD6-484F-8BC7-2C654C9A9B6F}")
    _methods_ = [
        COMMETHOD([], HRESULT, "GetSessionEnumerator", (["out"], POINTER(POINTER(IAudioSessionEnumerator)), "SessionEnum")),
    ]


_ole32 = ctypes.WinDLL("ole32")
_ole32.CoTaskMemFree.argtypes = [c_void_p]
_ole32.PropVariantClear.argtypes = [c_void_p]
_kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
_kernel32.OpenProcess.restype = wintypes.HANDLE
_kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
_kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
_kernel32.GetPackageFamilyName.argtypes = [wintypes.HANDLE, POINTER(wintypes.UINT), wintypes.LPWSTR]
PROCESS_QUERY_LIMITED_INFORMATION = 0x1000


def _take_string(p: c_void_p | int | None) -> str:
    if not p:
        return ""
    try:
        return ctypes.wstring_at(p)
    finally:
        _ole32.CoTaskMemFree(p)


def _enumerator() -> IMMDeviceEnumerator:
    return comtypes.CoCreateInstance(CLSID_MMDEVICE_ENUMERATOR, IMMDeviceEnumerator, CLSCTX_ALL)


def _device_id(dev: IMMDevice) -> str:
    return _take_string(dev.GetId())


def _device_name(dev: IMMDevice) -> str:
    store = dev.OpenPropertyStore(0)
    pv = PROPVARIANT_PTR()
    store.GetValue(byref(PKEY_DEVICE_FRIENDLY_NAME), byref(pv))
    try:
        return ctypes.wstring_at(pv.ptr) if pv.ptr else ""
    finally:
        _ole32.PropVariantClear(byref(pv))


def package_family(pid: int) -> str | None:
    h = _kernel32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid)
    if not h:
        return None
    try:
        n = wintypes.UINT(0)
        _kernel32.GetPackageFamilyName(h, byref(n), None)
        if not n.value:
            return None
        buf = ctypes.create_unicode_buffer(n.value)
        if _kernel32.GetPackageFamilyName(h, byref(n), buf) != 0:
            return None
        return buf.value or None
    finally:
        _kernel32.CloseHandle(h)


def key_for_pid(pid: int) -> str | None:
    """Clave del programa (como en el registro del micrófono). Los procesos auxiliares sin nombre
    propio (p. ej. WebView2) toman la de su app padre si esa es una plataforma de llamadas."""
    import psutil

    try:
        proc = psutil.Process(pid)
        key = process_key(proc.name(), package_family(pid))
        if key and platform_for_key(key):
            return key
        for parent in proc.parents()[:4]:
            k = process_key(parent.name(), package_family(parent.pid))
            if k and platform_for_key(k):
                return k
        return key
    except Exception:  # noqa: BLE001 - el proceso terminó, sin permisos…
        return None


@dataclass
class Snapshot:
    sessions: list[SessionInfo] = field(default_factory=list)
    names: dict[str, str] = field(default_factory=dict)  # id del dispositivo -> nombre
    comm: str | None = None  # predeterminado de comunicaciones
    console: str | None = None  # predeterminado general


def _default(enum: IMMDeviceEnumerator, flow: int, role: int) -> str | None:
    try:
        return _device_id(enum.GetDefaultAudioEndpoint(flow, role))
    except Exception:  # noqa: BLE001 - sin dispositivos de ese tipo
        return None


def _snapshot(flow: int) -> Snapshot:
    enum = _enumerator()
    snap = Snapshot(comm=_default(enum, flow, E_COMMUNICATIONS), console=_default(enum, flow, E_CONSOLE))
    coll = enum.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)
    keys: dict[int, str | None] = {}
    for i in range(coll.GetCount()):
        dev = coll.Item(i)
        dev_id = _device_id(dev)
        try:
            snap.names[dev_id] = _device_name(dev)
        except Exception:  # noqa: BLE001
            snap.names[dev_id] = dev_id
        try:
            mgr = dev.Activate(byref(IAudioSessionManager2._iid_), CLSCTX_ALL, None).QueryInterface(IAudioSessionManager2)
            sessions = mgr.GetSessionEnumerator()
            count = sessions.GetCount()
        except Exception:  # noqa: BLE001
            log.debug("Sin sesiones en %s", snap.names[dev_id], exc_info=True)
            continue
        for j in range(count):
            try:
                ctl = sessions.GetSession(j).QueryInterface(IAudioSessionControl2)
                pid = int(ctl.GetProcessId())
                state = ctl.GetState()
            except Exception:  # noqa: BLE001
                continue
            if not pid:
                continue  # sonidos del sistema
            if pid not in keys:
                keys[pid] = key_for_pid(pid)
            snap.sessions.append(SessionInfo(dev_id, snap.names[dev_id], pid,
                                             state == AUDIO_SESSION_STATE_ACTIVE, keys[pid]))
    return snap


def _in_mta(fn, timeout: float = 5.0):
    """Core Audio necesita COM: se usa en un hilo propio (el que llama puede ser de la ventana)."""
    out: list = []
    err: list[BaseException] = []

    def run():
        comtypes.CoInitializeEx(comtypes.COINIT_MULTITHREADED)
        try:
            out.append(fn())
        except BaseException as e:  # noqa: BLE001
            err.append(e)
        finally:
            comtypes.CoUninitialize()

    t = threading.Thread(target=run, name="coreaudio", daemon=True)
    t.start()
    t.join(timeout)
    if err:
        raise err[0]
    if not out:
        raise TimeoutError("Core Audio no respondió")
    return out[0]


def render_snapshot() -> Snapshot:
    return _in_mta(lambda: _snapshot(E_RENDER))


def capture_snapshot() -> Snapshot:
    return _in_mta(lambda: _snapshot(E_CAPTURE))


def capture_keys() -> set[str]:
    """Programas grabando de algún micrófono ahora mismo (claves como las del registro)."""
    return {s.key for s in capture_snapshot().sessions if s.active and s.key}


def activate_endpoint(endpoint_id: str, iid: GUID) -> IUnknown:
    """Abre una interfaz (p. ej. IAudioClient) sobre un dispositivo concreto. En un hilo con COM."""
    dev = _enumerator().GetDevice(endpoint_id)
    return dev.Activate(byref(iid), CLSCTX_ALL, None)
