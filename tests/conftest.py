import os

# Use offscreen Qt platform so UI tests run without a display
os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
