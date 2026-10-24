import multiprocessing

from calltx.main import main

if __name__ == "__main__":
    multiprocessing.freeze_support()
    raise SystemExit(main())
