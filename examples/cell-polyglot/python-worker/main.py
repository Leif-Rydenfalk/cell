import sys
import os

# Import local SDK (in a real setup, this would be `pip install cell-sdk`)
sys.path.append(os.path.abspath("../../../cell-py"))
from cell import Membrane

def main():
    print("Python Text Processor Starting...")
    
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