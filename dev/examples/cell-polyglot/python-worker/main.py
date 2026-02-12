import sys
import os

# Get the directory where this script resides (e.g., .../python-worker)
script_dir = os.path.dirname(os.path.abspath(__file__))

# Navigate relative to the script, not the CWD
# python-worker (0) -> cell-polyglot (1) -> examples (2) -> cell-root (3) -> cell-py
sdk_path = os.path.abspath(os.path.join(script_dir, "../../../cell-py"))
sys.path.append(sdk_path)

from cell import Membrane

def main():
    print(f"Python Text Processor Starting... (SDK Path: {sdk_path})")
    
    def handle_request(req):
        text = req.get("text", "")
        print(f"Processing: {text}")
        
        return {
            "original": text,
            "reversed": text[::-1],
            "uppercase": text.upper(),
            "processed_by": "Python 3.10"
        }

    m = Membrane()
    m.bind(handle_request)

if __name__ == "__main__":
    main()