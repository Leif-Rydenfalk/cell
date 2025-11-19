proper error handling can be incorporated and better end to end communication and failure mechanisms and progressive failure messages and the performance issues you mentioned are easy fixes. i hate wasm because i want to be able to multithread and simd each component individually. 

Think of the alternatives here: scripting with Rhai. if the scripts are instead written in pure modular and horizontally scalable rust imagine the possibilities.

what if we make a submodule in the game which creates new gameplay to find new creative ways to kill the player during runtime? can you do that with unity or wasm? we want baremetal control with the safety of rust and hot swappability of python.

we need to keep track of version history for cells and schemas