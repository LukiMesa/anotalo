"""Captura de lo que suena, con WASAPI loopback:

- ProcessLoopbackSource: el audio de UN proceso (y sus hijos), Windows 10 2004+/11. "Otros" solo
  contiene el audio de Zoom/Firefox…, no notificaciones ni otras apps, sea cual sea la salida.
- EndpointLoopbackSource: todo lo que suena en UN dispositivo de salida concreto (el que usa la
  llamada, p. ej. los auriculares Bluetooth), leído en el momento.
"""

from __future__ import annotations

import ctypes
import logging
import threading
import time
from ctypes import POINTER, byref, c_int, c_longlong, c_uint32, c_uint64, c_ushort, c_void_p, wintypes

import comtypes
import numpy as np
from comtypes import COMMETHOD, GUID, HRESULT, IUnknown

from .base import OnAudio
from .resample import to_mono_16k

log = logging.getLogger(__name__)

SR = 48000
CHANNELS = 2

AUDCLNT_SHAREMODE_SHARED = 0
AUDCLNT_STREAMFLAGS_LOOPBACK = 0x00020000
AUDCLNT_STREAMFLAGS_EVENTCALLBACK = 0x00040000
AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM = 0x80000000
AUDCLNT_BUFFERFLAGS_SILENT = 0x2
AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK = 1
PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE = 0
VT_BLOB = 65
VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK = "VAD\\Process_Loopback"


class WAVEFORMATEX(ctypes.Structure):
    _fields_ = [
        ("wFormatTag", wintypes.WORD),
        ("nChannels", wintypes.WORD),
        ("nSamplesPerSec", wintypes.DWORD),
        ("nAvgBytesPerSec", wintypes.DWORD),
        ("nBlockAlign", wintypes.WORD),
        ("wBitsPerSample", wintypes.WORD),
        ("cbSize", wintypes.WORD),
    ]


class AUDIOCLIENT_ACTIVATION_PARAMS(ctypes.Structure):
    _fields_ = [
        ("ActivationType", c_int),
        ("TargetProcessId", wintypes.DWORD),
        ("ProcessLoopbackMode", c_int),
    ]


class BLOB(ctypes.Structure):
    _fields_ = [("cbSize", wintypes.ULONG), ("pBlobData", c_void_p)]


class PROPVARIANT(ctypes.Structure):
    _fields_ = [
        ("vt", c_ushort),
        ("wReserved1", c_ushort),
        ("wReserved2", c_ushort),
        ("wReserved3", c_ushort),
        ("blob", BLOB),
    ]


class IAudioCaptureClient(IUnknown):
    _iid_ = GUID("{C8ADBD64-E71E-48a0-A4DE-185C395CD317}")
    _methods_ = [
        COMMETHOD([], HRESULT, "GetBuffer",
                  (["out"], POINTER(POINTER(ctypes.c_ubyte)), "ppData"),
                  (["out"], POINTER(c_uint32), "pNumFramesToRead"),
                  (["out"], POINTER(wintypes.DWORD), "pdwFlags"),
                  (["out"], POINTER(c_uint64), "pu64DevicePosition"),
                  (["out"], POINTER(c_uint64), "pu64QPCPosition")),
        COMMETHOD([], HRESULT, "ReleaseBuffer", (["in"], c_uint32, "NumFramesRead")),
        COMMETHOD([], HRESULT, "GetNextPacketSize", (["out"], POINTER(c_uint32), "pNumFramesInNextPacket")),
    ]


class IAudioClient(IUnknown):
    _iid_ = GUID("{1CB9AD4C-DBFA-4c32-B178-C2F568A703B2}")
    _methods_ = [
        COMMETHOD([], HRESULT, "Initialize",
                  (["in"], c_int, "ShareMode"),
                  (["in"], wintypes.DWORD, "StreamFlags"),
                  (["in"], c_longlong, "hnsBufferDuration"),
                  (["in"], c_longlong, "hnsPeriodicity"),
                  (["in"], POINTER(WAVEFORMATEX), "pFormat"),
                  (["in"], POINTER(GUID), "AudioSessionGuid")),
        COMMETHOD([], HRESULT, "GetBufferSize", (["out"], POINTER(c_uint32), "pNumBufferFrames")),
        COMMETHOD([], HRESULT, "GetStreamLatency", (["out"], POINTER(c_longlong), "phnsLatency")),
        COMMETHOD([], HRESULT, "GetCurrentPadding", (["out"], POINTER(c_uint32), "pNumPaddingFrames")),
        COMMETHOD([], HRESULT, "IsFormatSupported",
                  (["in"], c_int, "ShareMode"),
                  (["in"], POINTER(WAVEFORMATEX), "pFormat"),
                  (["out"], POINTER(POINTER(WAVEFORMATEX)), "ppClosestMatch")),
        COMMETHOD([], HRESULT, "GetMixFormat", (["out"], POINTER(POINTER(WAVEFORMATEX)), "ppDeviceFormat")),
        COMMETHOD([], HRESULT, "GetDevicePeriod",
                  (["out"], POINTER(c_longlong), "phnsDefaultDevicePeriod"),
                  (["out"], POINTER(c_longlong), "phnsMinimumDevicePeriod")),
        COMMETHOD([], HRESULT, "Start"),
        COMMETHOD([], HRESULT, "Stop"),
        COMMETHOD([], HRESULT, "Reset"),
        COMMETHOD([], HRESULT, "SetEventHandle", (["in"], wintypes.HANDLE, "eventHandle")),
        COMMETHOD([], HRESULT, "GetService",
                  (["in"], POINTER(GUID), "riid"),
                  (["out"], POINTER(POINTER(IUnknown)), "ppv")),
    ]


class IActivateAudioInterfaceAsyncOperation(IUnknown):
    _iid_ = GUID("{72A22D78-CDE4-431D-B8CC-843A71199B6D}")
    _methods_ = [
        COMMETHOD([], HRESULT, "GetActivateResult",
                  (["out"], POINTER(HRESULT), "activateResult"),
                  (["out"], POINTER(POINTER(IUnknown)), "activatedInterface")),
    ]


class IActivateAudioInterfaceCompletionHandler(IUnknown):
    _iid_ = GUID("{41D949AB-9862-444A-80F6-C261334DA5EB}")
    _methods_ = [
        COMMETHOD([], HRESULT, "ActivateCompleted",
                  (["in"], POINTER(IActivateAudioInterfaceAsyncOperation), "activateOperation")),
    ]


class IAgileObject(IUnknown):
    _iid_ = GUID("{94EA2B94-E9CC-49E0-C0FF-EE64CA8F5B90}")
    _methods_ = []


class _Handler(comtypes.COMObject):
    _com_interfaces_ = [IActivateAudioInterfaceCompletionHandler, IAgileObject]

    def __init__(self):
        super().__init__()
        self.done = threading.Event()

    def ActivateCompleted(self, operation):  # noqa: N802 - nombre COM
        self.done.set()
        return 0


_kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
_kernel32.CreateEventW.restype = wintypes.HANDLE
_kernel32.CreateEventW.argtypes = [c_void_p, wintypes.BOOL, wintypes.BOOL, wintypes.LPCWSTR]
_kernel32.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
_kernel32.CloseHandle.argtypes = [wintypes.HANDLE]


def _activate(pid: int) -> IAudioClient:
    """Debe llamarse en un hilo MTA."""
    mmdevapi = ctypes.WinDLL("Mmdevapi")
    fn = mmdevapi.ActivateAudioInterfaceAsync
    fn.restype = HRESULT
    fn.argtypes = [
        wintypes.LPCWSTR,
        POINTER(GUID),
        POINTER(PROPVARIANT),
        POINTER(IActivateAudioInterfaceCompletionHandler),
        POINTER(POINTER(IActivateAudioInterfaceAsyncOperation)),
    ]
    params = AUDIOCLIENT_ACTIVATION_PARAMS(
        AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, pid, PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE
    )
    pv = PROPVARIANT()
    pv.vt = VT_BLOB
    pv.blob.cbSize = ctypes.sizeof(params)
    pv.blob.pBlobData = ctypes.cast(byref(params), c_void_p)

    handler = _Handler()
    op = POINTER(IActivateAudioInterfaceAsyncOperation)()
    iid = IAudioClient._iid_
    fn(
        VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
        byref(iid),
        byref(pv),
        handler.QueryInterface(IActivateAudioInterfaceCompletionHandler),
        byref(op),
    )
    if not handler.done.wait(5):
        raise TimeoutError("ActivateAudioInterfaceAsync no respondió")
    hr, unk = op.GetActivateResult()
    if hr < 0:
        raise OSError(f"Activación de process loopback falló: HRESULT 0x{hr & 0xFFFFFFFF:08X}")
    return unk.QueryInterface(IAudioClient)


def _format() -> WAVEFORMATEX:
    return WAVEFORMATEX(1, CHANNELS, SR, SR * CHANNELS * 2, CHANNELS * 2, 16, 0)


class ProcessLoopbackSource:
    name = "loopback-process"

    def __init__(self, pid: int):
        self.pid = pid
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None
        self._ready = threading.Event()
        self._error: BaseException | None = None

    def probe(self) -> None:
        """Comprueba que la activación funciona (lanza excepción si no)."""
        err: list[BaseException] = []

        def run():
            comtypes.CoInitializeEx(comtypes.COINIT_MULTITHREADED)
            try:
                client = self._activate()
                fmt = _format()
                client.Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
                    2_000_000, 0, byref(fmt), None,
                )
                del client
            except BaseException as e:  # noqa: BLE001
                err.append(e)
            finally:
                comtypes.CoUninitialize()

        t = threading.Thread(target=run, daemon=True)
        t.start()
        t.join(10)
        if err:
            raise err[0]
        if t.is_alive():
            raise TimeoutError("probe de process loopback colgado")

    def start(self, on_audio: OnAudio) -> None:
        self._stop.clear()
        self._ready.clear()
        self._error = None
        self._thread = threading.Thread(target=self._run, args=(on_audio,), name="proc-loopback", daemon=True)
        self._thread.start()
        self._ready.wait(10)
        if self._error is not None:
            raise self._error

    def stop(self) -> None:
        self._stop.set()
        if self._thread is not None:
            self._thread.join(3)
            self._thread = None

    def _activate(self) -> IAudioClient:
        return _activate(self.pid)

    def describe(self) -> str:
        return f"el audio del proceso {self.pid}"

    def is_active(self) -> bool:
        import psutil

        return self._thread is not None and self._thread.is_alive() and psutil.pid_exists(self.pid)

    def _run(self, on_audio: OnAudio) -> None:
        comtypes.CoInitializeEx(comtypes.COINIT_MULTITHREADED)
        event = None
        client = None
        try:
            client = self._activate()
            fmt = _format()
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
                2_000_000, 0, byref(fmt), None,
            )
            event = _kernel32.CreateEventW(None, False, False, None)
            client.SetEventHandle(event)
            capture = client.GetService(byref(IAudioCaptureClient._iid_)).QueryInterface(IAudioCaptureClient)
            client.Start()
            log.info("Capturando %s", self.describe())
        except BaseException as e:  # noqa: BLE001
            self._error = e
            self._ready.set()
            comtypes.CoUninitialize()
            return
        self._ready.set()
        block = CHANNELS * 2
        try:
            while not self._stop.is_set():
                _kernel32.WaitForSingleObject(event, 100)
                chunks: list[bytes] = []
                frames_total = 0
                while capture.GetNextPacketSize() > 0:
                    data, frames, flags, _pos, _qpc = capture.GetBuffer()
                    if frames:
                        if flags & AUDCLNT_BUFFERFLAGS_SILENT:
                            chunks.append(bytes(frames * block))
                        else:
                            chunks.append(ctypes.string_at(data, frames * block))
                        frames_total += frames
                    capture.ReleaseBuffer(frames)
                if frames_total:
                    now = time.monotonic()
                    x = np.frombuffer(b"".join(chunks), dtype=np.int16)
                    on_audio(now - frames_total / SR, to_mono_16k(x, SR, CHANNELS))
        except Exception:  # noqa: BLE001
            log.warning("Captura de %s interrumpida", self.describe(), exc_info=True)
        finally:
            try:
                client.Stop()
            except Exception:  # noqa: BLE001
                pass
            if event:
                _kernel32.CloseHandle(event)
            del capture, client
            comtypes.CoUninitialize()


class EndpointLoopbackSource(ProcessLoopbackSource):
    """Todo lo que suena en un dispositivo de salida concreto (por su id de Windows)."""

    name = "loopback-endpoint"

    def __init__(self, endpoint_id: str, endpoint_name: str = ""):
        super().__init__(0)
        self.endpoint_id = endpoint_id
        self.endpoint_name = endpoint_name or endpoint_id

    def _activate(self) -> IAudioClient:
        from .coreaudio import activate_endpoint

        return activate_endpoint(self.endpoint_id, IAudioClient._iid_).QueryInterface(IAudioClient)

    def describe(self) -> str:
        return f"lo que suena en {self.endpoint_name}"

    def is_active(self) -> bool:
        # si el dispositivo desaparece (auriculares apagados), el hilo termina y la sesión reabre
        return self._thread is not None and self._thread.is_alive()
