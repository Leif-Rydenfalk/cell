Similar to godot in architecture but each mod is a cell.

What if I create a new type of entity with a compeltely new render pipeline with shaders and new memory types - how does it handle this and how does the rest of the app interact with it?
What if for each entity we want to render we have one cell which talks to the engine and keeps track of all of the memory etc of this entity. then when other cells needs this entity - for example this entity might be ui buttons - we then talk to the cell which holds that specific entity to get to know this entity.

The cell cli is installed. 
I dont want to have to manually compile the project. cell mitosis . should automatically do everything for me - it should just work. Also - why do we have to define axons? Doesnt it auto discover?

Cell is under heavy development so we got to fix every issue we find.
Im developing this game engine both as an example on how to use Cell for other people and as a project to refine the Cell api to be as nice as possible and add all features I need along the way.

We should not have to define dependency / boot order. It should all be done for us.
There should not be a seperate cell clean command. Only a cell mitosis . command which does everything for us. It must detect zombie processes. The sdk perhaps should send a kill message to let other processes and the cli / daemon know that its dead.

When modifying source code - always keep previous features and comments!

We need recursive service discovery in favor of the topological graph resolution - but a hybrid approach would be the best. 
Cell makes it very easy to make your game multiplayer - you simply host the stuff elsewhere. thats it. one single config change. with purely topological this is not possible. 
Cell is also used for microservices as a replacement to kubernetes in my backends. 

The cell-engine is ONLY an example on how to use cell! I will use cell for my backend infrastructure and banking systems later. Keep this in mind when modifying anything.


i hate that we bill cpu time in a local project. cell has multiple purposes:
1: be a good easy to use microservices kernel + infrastructure composer.
2: Global supercomputer.

They are so close in features but I dont want to have the billing logic for every single cell inside a game engine / game if its not running globally. We need to think about this. What if each cell by default meassures its cpu and gpu and memory very lightweight and we use this as metrics in the game engine, or for the billing in the supercomputer?




The cells should manage everything themselves but there should be one small cell which every cell has a connection to so that they are able to talk to eachother. Does this make sense?
This one cell - the "orchestrator" or whatever is only once per machine - this is the daemon which the cell-cli accesses.


There should never be scanning because if a cell wants to be seen it will make sure its seen by letting all other cells around it know its there.


If I make my own game engine where each cell = a feature in the game engine / a system and they can rely on eachother and I can do literally anything you can do in rust + wgpu but I can do it together and coherent and feed to ai to build huge projects - what would this engine architecture look like? Would you provide a cell which provides your game / feature code and adds UI + does all rendering to the app so you can just add more stuff? Basically an operating system but global and for games and AI
How will the architecture of the renderer work?