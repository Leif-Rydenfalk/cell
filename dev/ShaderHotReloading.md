# Shader Hot Reloading Implementation Guide

## Setup Steps

1. **Create Shader Directory**:
   ```bash
   mkdir -p src/shaders
   ```

2. **Extract Embedded Shaders to Files**:
   - Create `src/shaders/bloom.wgsl` from your existing shader
   - Create `src/shaders/color_correction.wgsl` from your existing shader
   - Create `src/shaders/final_conversion.wgsl` from your existing shader

3. **Add Changes to Your Project**:
   - Replace the old `wgpu_ctx.rs` with the new version
   - Replace the old `color_correction.rs` with the new version

## How It Works

1. The `ShaderHotReload` struct:
   - Keeps track of loaded shaders and their last modification times
   - Loads shaders from disk when requested
   - Checks for updates on a timer
   - Falls back to embedded shaders if files aren't found

2. Changes to constructors:
   - Post-processing effects now accept custom shaders
   - Pipelines are recreated with updated shaders

3. Usage flow:
   - During startup, shaders are loaded from disk
   - During the render loop, `check_shader_updates()` is called
   - If a shader file changes, it's reloaded and pipelines are recreated
   - A message is printed to the console on successful reload

## Added Features

1. **ImGui Integration**:
   - A new "Force Reload All Shaders" button in the UI
   - This is useful for debugging or when automatic reloading isn't working

2. **Fallback Mechanism**:
   - If shader files can't be found, falls back to embedded shaders
   - This ensures your application still works without external files

3. **Console Logging**:
   - Messages are printed when shaders are loaded or reloaded
   - This helps with debugging shader issues

## Tips for Shader Development

1. **Make Small Changes**:
   - Start with simple modifications to verify hot reloading works
   - Save the file and watch for the "Reloaded" message

2. **Watch the Console**:
   - Keep an eye on console output for shader loading messages
   - If the shader doesn't reload, check file paths and permissions

3. **Use the Force Reload Button**:
   - If automatic reloading isn't working, try the force reload button
   - This bypasses the timestamp check and reloads all shaders

## Common Issues

1. **Incorrect Paths**:
   - The code expects shaders in `src/shaders/`, adjust if your structure differs
   - Check that file names match exactly what the code is looking for

2. **Shader Compilation Errors**:
   - If a shader fails to compile, the old one remains in use
   - Check the console for compiler errors

3. **No Reload Notification**:
   - The system only checks once per second, so wait a moment
   - Verify file modification times are updating when you save