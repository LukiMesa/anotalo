"""Ventana con la transcripción en vivo (tkinter)."""

from __future__ import annotations

import tkinter as tk
from tkinter import ttk
from typing import Callable

COLORS = {"me": "#1f6feb", "others": "#b3261e"}


class LiveWindow:
    def __init__(self, root: tk.Tk, label_me: str, get_text: Callable[[], str]):
        self.root = root
        self._label_me = label_me
        self._get_text = get_text
        root.title("Transcripción de llamada")
        root.geometry("560x640")
        root.protocol("WM_DELETE_WINDOW", self.hide)

        top = ttk.Frame(root, padding=(8, 6))
        top.pack(fill="x")
        self._status = tk.StringVar(value="Inactivo")
        ttk.Label(top, textvariable=self._status, font=("Segoe UI", 10, "bold")).pack(side="left")
        ttk.Button(top, text="Copiar todo", command=self._copy).pack(side="right")

        frame = ttk.Frame(root)
        frame.pack(fill="both", expand=True)
        self._text = tk.Text(frame, wrap="word", font=("Segoe UI", 11), padx=10, pady=8, state="disabled", borderwidth=0)
        sb = ttk.Scrollbar(frame, command=self._text.yview)
        self._text.configure(yscrollcommand=sb.set)
        sb.pack(side="right", fill="y")
        self._text.pack(side="left", fill="both", expand=True)
        self._text.tag_configure("me", foreground=COLORS["me"], font=("Segoe UI", 11, "bold"))
        self._text.tag_configure("others", foreground=COLORS["others"], font=("Segoe UI", 11, "bold"))
        self._text.tag_configure("ts", foreground="#888888")
        root.withdraw()

    # Todos los métodos públicos son seguros desde cualquier hilo.
    def append(self, speaker: str, ts: str, text: str) -> None:
        self.root.after(0, self._append, speaker, ts, text)

    def _append(self, speaker: str, ts: str, text: str) -> None:
        at_bottom = self._text.yview()[1] > 0.98
        self._text.configure(state="normal")
        self._text.insert("end", f"[{ts}] ", "ts")
        self._text.insert("end", f"{speaker}: ", "me" if speaker == self._label_me else "others")
        self._text.insert("end", text + "\n\n")
        self._text.configure(state="disabled")
        if at_bottom:
            self._text.see("end")

    def clear(self, status: str) -> None:
        def do():
            self._text.configure(state="normal")
            self._text.delete("1.0", "end")
            self._text.configure(state="disabled")
            self._status.set(status)

        self.root.after(0, do)

    def set_status(self, text: str) -> None:
        self.root.after(0, self._status.set, text)

    def show(self) -> None:
        def do():
            self.root.deiconify()
            self.root.lift()
            self.root.focus_force()

        self.root.after(0, do)

    def hide(self) -> None:
        self.root.withdraw()

    def _copy(self) -> None:
        self.root.clipboard_clear()
        self.root.clipboard_append(self._get_text())
