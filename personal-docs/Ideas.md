You run the project using cargo run --release or cell mitosis
Then you go to the cell directory and run cell swap and it will swap the version you are working on in the running project.

how hard is it to design a monitoring cell which looks at messages and events and logs from all cells and isolate problems and which cells they occur in. Then we use an llm ai model to try to fix the issue, write several tests and then hot swap in the cell into the already running system.

This architecture is actually designed specifically for ai models because each cell has a fixed input and output we dont have to provide a whole codebase to ai. This is how I got the idea in the first place. 

We can build systems which builds themselves without any human intervention at a global scale.

Say we have two cells which constantly tries to write better versions of the other one and they split into more and more subsystems with more and more specific and sophisticated goals.