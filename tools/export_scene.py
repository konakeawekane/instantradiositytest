"""Blender background exporter. Mesh positions/normals are Z-up, camera looks -Z.

Geometry: little-endian 64-byte vertices (position4, normal4, tangent4, uv4),
followed by uint32 indices. SDF: x-fastest float32 signed samples on grid nodes.
The runtime subtracts a voxel interpolation bound before conservative stepping.
"""
import argparse
import array
import hashlib
import json
import math
from pathlib import Path
import struct
import sys
import uuid

import bpy
import bmesh
from mathutils import Vector, Matrix
from mathutils.bvhtree import BVHTree


def require(condition, message):
    if not condition:
        raise ValueError(message)


def texture_source(socket, material, normal=False, uv_map=None, scalar=False):
    if not socket.is_linked:
        return None, 0, 1.0
    link = socket.links[0]
    node = link.from_node
    channel, strength = 0, 1.0
    if normal:
        require(node.type == 'NORMAL_MAP' and node.space == 'TANGENT', f'{material}: Normal must use a tangent Normal Map node')
        require(not node.uv_map or node.uv_map==uv_map,f'{material}: Normal Map must use the active UV map')
        require(not node.inputs['Strength'].is_linked, f'{material}: linked normal strength is unsupported')
        strength = node.inputs['Strength'].default_value
        require(node.inputs['Color'].is_linked, f'{material}: Normal Map has no image')
        link = node.inputs['Color'].links[0]
        node = link.from_node
    if node.type in ('SEPRGB', 'SEPARATE_COLOR'):
        require(scalar, f'{material}: color and normal inputs require the full image Color output')
        require(node.type == 'SEPRGB' or node.mode == 'RGB', f'{material}: channel separator must use RGB mode')
        channel = ['Red', 'Green', 'Blue'].index(link.from_socket.name)
        input_socket = node.inputs[0]
        require(input_socket.is_linked, f'{material}: channel separator has no image')
        link = input_socket.links[0]
        require(link.from_socket.name == 'Color', f'{material}: channel separator requires the image Color output')
        node = link.from_node
    elif scalar and link.from_socket.name == 'Alpha':
        channel = 3
    else:
        require(link.from_socket.name == 'Color', f'{material}: color and normal inputs require the image Color output')
    require(node.type == 'TEX_IMAGE' and node.image is not None, f'{material}: only constants and image texture inputs are supported ({socket.name}: {node.type})')
    require(node.extension == 'REPEAT', f'{material}: image extension must be REPEAT')
    require(node.interpolation == 'Linear', f'{material}: image interpolation must be Linear')
    if node.inputs['Vector'].is_linked:
        src = node.inputs['Vector'].links[0]
        require(src.from_node.type == 'UVMAP' or (src.from_node.type == 'TEX_COORD' and src.from_socket.name == 'UV'), f'{material}: texture coordinates must use the active UV map')
        if src.from_node.type=='UVMAP':require(not src.from_node.uv_map or src.from_node.uv_map==uv_map,f'{material}: image must use the active UV map')
    require(uv_map is not None,f'{material}: textured mesh requires an active UV map')
    return node.image, channel, strength


def export_material(mat, output, uv_map=None):
    name = mat.name if mat else '__default'
    result = dict(id=name, base_color=[0.8, 0.8, 0.8, 1.0], roughness=0.5, metallic=0.0, normal_strength=1.0, base_color_srgb=True, texture_channels=[0, 0, 0, 0])
    for channel in ('base_color', 'roughness', 'metallic', 'normal'):
        result[channel + '_texture'] = None
    if not mat:
        return result
    if not mat.use_nodes:
        result.update(base_color=list(mat.diffuse_color), roughness=mat.roughness, metallic=mat.metallic)
        return result
    outputs = [n for n in mat.node_tree.nodes if n.type == 'OUTPUT_MATERIAL' and n.is_active_output]
    require(len(outputs) == 1 and outputs[0].inputs['Surface'].is_linked, f'{name}: no active material output')
    require(not outputs[0].inputs['Volume'].is_linked and not outputs[0].inputs['Displacement'].is_linked,f'{name}: volume and shader displacement are unsupported; bake geometry modifiers instead')
    bsdf = outputs[0].inputs['Surface'].links[0].from_node
    require(bsdf.type == 'BSDF_PRINCIPLED', f'{name}: requires Principled BSDF')
    alpha=bsdf.inputs['Alpha'];require(not alpha.is_linked and alpha.default_value==1,f'{name}: only opaque materials are supported')
    for key in ('Transmission Weight', 'Coat Weight', 'Sheen Weight', 'Subsurface Weight', 'Anisotropic IOR Level', 'Diffuse Roughness', 'Thin Film Thickness'):
        s = bsdf.inputs.get(key)
        require(s is None or (not s.is_linked and s.default_value == 0), f'{name}: unsupported {key}')
    for key,default in (('IOR',1.5),('Specular IOR Level',0.5)):
        s=bsdf.inputs.get(key)
        require(s is None or (not s.is_linked and abs(s.default_value-default)<1e-6),f'{name}: renderer uses fixed dielectric F0=0.04; unsupported {key}')
    emission=bsdf.inputs['Emission Color'];strength=bsdf.inputs['Emission Strength']
    require(not emission.is_linked and not strength.is_linked and (strength.default_value==0 or max(emission.default_value[:3])==0),f'{name}: unsupported emission')
    for i, (field, label) in enumerate(zip(('base_color', 'roughness', 'metallic', 'normal'), ('Base Color', 'Roughness', 'Metallic', 'Normal'))):
        socket = bsdf.inputs[label]
        image, component, strength = texture_source(socket, name, field == 'normal',uv_map,field in ('roughness','metallic'))
        if image:
            if field=='base_color':
                require(image.colorspace_settings.name in ('sRGB','Non-Color','Linear Rec.709'),f'{name}: unsupported base color space {image.colorspace_settings.name}')
                result['base_color_srgb']=image.colorspace_settings.name=='sRGB'
            else:require(image.colorspace_settings.name=='Non-Color',f'{name}: {field} texture must be Non-Color data')
            # Save a copy without changing the source .blend or applying its view transform.
            pixels = array.array('f', [0]) * len(image.pixels)
            image.pixels.foreach_get(pixels)
            if field in ('roughness','metallic') and socket.links[0].from_node.type == 'TEX_IMAGE' and socket.links[0].from_socket.name == 'Color':
                require(all(abs(pixels[i]-pixels[i+1]) < 1e-6 and abs(pixels[i]-pixels[i+2]) < 1e-6 for i in range(0,len(pixels),4)),f'{name}: direct scalar Color image must be grayscale; use Separate Color for a packed RGB channel')
            image_metadata = (image.name,image.filepath,image.channels,image.alpha_mode,image.colorspace_settings.name)
            path = output / 'textures' / (hashlib.sha256(repr(image_metadata).encode() + pixels.tobytes()).hexdigest()[:16] + '.png')
            if not path.exists():
                copy = image.copy()
                copy.pixels.foreach_set(pixels)
                copy.update()
                copy.filepath_raw = str(path); copy.file_format = 'PNG'; copy.save()
                bpy.data.images.remove(copy)
            result[field + '_texture'] = str(path.relative_to(output)).replace('\\', '/')
            result['texture_channels'][i] = component
            if field == 'base_color': result[field] = [1, 1, 1, 1]
            elif field in ('roughness', 'metallic'): result[field] = 1.0
            else: result['normal_strength'] = strength
        elif field == 'base_color': result[field] = list(socket.default_value)
        elif field != 'normal': result[field] = socket.default_value
    return result


def inside(bvh, point, extent):
    # Majority ray parity avoids ambiguous edge/vertex intersections.
    votes = 0
    for direction in (Vector((1, .371, .129)).normalized(), Vector((.173, 1, .419)).normalized(), Vector((.319, .217, 1)).normalized()):
        start, hits = point.copy(), 0
        for _ in range(256):
            loc, _, _, _ = bvh.ray_cast(start, direction, extent * 4)
            if loc is None: break
            hits += 1; start = loc + direction * max(extent * 1e-6, 1e-7)
        else: raise ValueError('SDF classification exceeded 256 intersections')
        votes += hits % 2
    return votes >= 2


def build_sdf(vertices, triangles, resolution, path):
    lo = Vector(tuple(min(v[i] for v in vertices) for i in range(3)))
    hi = Vector(tuple(max(v[i] for v in vertices) for i in range(3)))
    padding = (hi - lo).length / (resolution - 1) * 2
    lo -= Vector((padding,) * 3); hi += Vector((padding,) * 3)
    bvh = BVHTree.FromPolygons(vertices, triangles, all_triangles=True)
    extent = (hi - lo).length
    key = hashlib.sha256(struct.pack('<I', resolution) + b''.join(struct.pack('<3f', *v) for v in vertices) + b''.join(struct.pack('<3I', *t) for t in triangles)).hexdigest()
    cache = path.parent / ('sdf_' + key + '.bin')
    if not cache.exists():
        data = array.array('f')
        for z in range(resolution):
            for y in range(resolution):
                for x in range(resolution):
                    p = Vector(tuple(lo[i] + (hi[i] - lo[i]) * c / (resolution - 1) for i, c in enumerate((x, y, z))))
                    _, _, _, distance = bvh.find_nearest(p)
                    sign = -1 if inside(bvh, p, extent) else 1
                    data.append(sign * distance - extent * 2e-6)
            if z % 16 == 0: print(f'SDF {path.stem}: {z}/{resolution}', flush=True)
        if sys.byteorder != 'little': data.byteswap()
        cache.write_bytes(data.tobytes())
    with path.open('ab') as f:
        offset = f.tell(); f.write(cache.read_bytes())
    return dict(path=path.name, offset=offset, resolution=resolution, bounds_min=list(lo), bounds_max=list(hi), conservative=True, query='trilinear_bound', error_bound=extent * 2e-6)


def main():
    args = argparse.ArgumentParser()
    args.add_argument('--output', required=True)
    args.add_argument('--sdf-resolution', type=int, default=48)
    opt = args.parse_args(sys.argv[sys.argv.index('--') + 1:])
    require(4 <= opt.sdf_resolution <= 256, 'SDF resolution must be in [4,256]')
    manifest = Path(opt.output).resolve(); output = manifest.parent
    output.mkdir(parents=True, exist_ok=True); (output / 'textures').mkdir(exist_ok=True)
    token=uuid.uuid4().hex
    geometry_path, sdf_path = output / ('meshes_'+token+'.tmp'), output / ('sdf_'+token+'.tmp')
    geometry_path.write_bytes(b''); sdf_path.write_bytes(b'')
    scene = dict(version=1, meshes=[], materials=[], objects=[], lights=[], camera={})
    meshes, materials = {}, {}
    dg = bpy.context.evaluated_depsgraph_get()
    for obj in sorted(bpy.context.scene.objects, key=lambda o: o.name):
        if obj.hide_render: continue
        if obj.type == 'MESH':
            evaluated = obj.evaluated_get(dg); mesh = evaluated.to_mesh(preserve_all_data_layers=True, depsgraph=dg)
            try:
                bm = bmesh.new(); bm.from_mesh(mesh)
                require(all(e.is_manifold and e.is_contiguous for e in bm.edges) and len(bm.faces) > 0, f'{obj.name}: mesh must be closed, watertight and consistently oriented')
                signed_volume = bm.calc_volume(signed=True)
                bmesh.ops.triangulate(bm, faces=list(bm.faces)); bm.to_mesh(mesh); bm.free()
                require(signed_volume > 0, f'{obj.name}: mesh volume must be positive; recalculate normals outside')
                pos, rot, scale = obj.matrix_world.decompose()
                require(min(scale) > 0, f'{obj.name}: negative/zero scale unsupported; apply transforms in Blender')
                reconstructed = Matrix.LocRotScale(pos, rot, scale)
                require(max(abs(obj.matrix_world[r][c] - reconstructed[r][c]) for r in range(4) for c in range(4)) < 1e-5, f'{obj.name}: shear transform unsupported')
                uniform = max(scale) - min(scale) < max(scale) * 1e-5
                bake = Vector((1, 1, 1)) if uniform else scale
                mesh.transform(Matrix.Diagonal((*bake, 1)))
                mesh.calc_loop_triangles()
                if mesh.uv_layers.active: mesh.calc_tangents()
                verts, indices = [], []
                for tri in mesh.loop_triangles:
                    for loop_index in tri.loops:
                        loop = mesh.loops[loop_index]; p = mesh.vertices[loop.vertex_index].co
                        normal = loop.normal; tangent = loop.tangent if mesh.uv_layers.active else Vector((1, 0, 0))
                        uv = mesh.uv_layers.active.data[loop_index].uv if mesh.uv_layers.active else (0, 0)
                        verts.append(struct.pack('<16f', *p, 1, *normal, 0, *tangent, loop.bitangent_sign if mesh.uv_layers.active else 1, *uv, 0, 0)); indices.append(len(indices))
                geometry = b''.join(verts) + struct.pack('<' + 'I' * len(indices), *indices)
                resolution = int(obj.get('sdf_resolution', obj.data.get('sdf_resolution', opt.sdf_resolution)))
                require(4 <= resolution <= 256, f'{obj.name}: sdf_resolution must be in [4,256]')
                key = hashlib.sha256(geometry + struct.pack('<I', resolution)).hexdigest()
                if key not in meshes:
                    mesh_id = obj.data.name + '_' + key[:10]
                    with geometry_path.open('ab') as f:
                        vo = f.tell(); f.write(geometry)
                    sdf = build_sdf([v.co.copy() for v in mesh.vertices], [tuple(t.vertices) for t in mesh.loop_triangles], resolution, sdf_path)
                    asset = dict(id=mesh_id, geometry=dict(path=geometry_path.name, vertex_offset=vo, vertex_count=len(verts), index_offset=vo + len(verts) * 64, index_count=len(indices)), sdf=sdf)
                    scene['meshes'].append(asset); meshes[key] = mesh_id
                used = {t.material_index for t in mesh.loop_triangles}
                require(len(used) <= 1, f'{obj.name}: one material per object is required by scene v1')
                mat = mesh.materials[next(iter(used))] if len(mesh.materials) else None
                material_id = mat.name if mat else '__default'
                uv_name=mesh.uv_layers.active.name if mesh.uv_layers.active else None
                # Validate each instance's UV layout, including shared materials.
                if mat and mat.use_nodes:
                    for n in mat.node_tree.nodes:
                        if n.type=='TEX_IMAGE' and n.image and any(socket.is_linked for socket in n.outputs):
                            require(uv_name is not None,f'{obj.name}: textured mesh requires an active UV map')
                            if n.inputs['Vector'].is_linked:
                                src=n.inputs['Vector'].links[0].from_node
                                if src.type=='UVMAP':require(not src.uv_map or src.uv_map==uv_name,f'{obj.name}: image must use the active UV map')
                        if n.type=='NORMAL_MAP':require(not n.uv_map or n.uv_map==uv_name,f'{obj.name}: Normal Map must use the active UV map')
                if material_id not in materials:
                    materials[material_id] = export_material(mat, output,uv_name); scene['materials'].append(materials[material_id])
                scene['objects'].append(dict(id=obj.name, mesh=meshes[key], material=material_id, transform=dict(position=list(pos), rotation=[rot.x, rot.y, rot.z, rot.w], scale=float(scale.x if uniform else 1))))
            finally: evaluated.to_mesh_clear()
        elif obj.type == 'LIGHT':
            light = obj.data
            require(light.type in ('POINT', 'SPOT'), f'{obj.name}: only point and spot lights supported')
            solid_angle = 4 * math.pi if light.type == 'POINT' else 2 * math.pi * (1 - math.cos(light.spot_size / 2))
            record = dict(id=obj.name, type=light.type.lower(), position=list(obj.matrix_world.translation), color=list(light.color), strength=light.energy / solid_angle)
            if light.type == 'SPOT': record.update(direction=list((obj.matrix_world.to_quaternion() @ Vector((0, 0, -1))).normalized()), angle_radians=light.spot_size)
            scene['lights'].append(record)
    camera = bpy.context.scene.camera
    require(camera is not None and camera.data.type == 'PERSP', 'An active perspective camera is required')
    pos, rot, _ = camera.matrix_world.decompose()
    aspect = 1920 / 1200
    fov_y = 2 * math.atan(math.tan(camera.data.angle_x / 2) / aspect)
    scene['camera'] = dict(position=list(pos), rotation=[rot.x, rot.y, rot.z, rot.w], fov_y_radians=fov_y, near=camera.data.clip_start, far=camera.data.clip_end)
    # Publish immutable, content-addressed payloads before atomically replacing
    # the manifest. A rejected export never damages a previous successful one.
    geometry_final=output/('meshes_'+hashlib.sha256(geometry_path.read_bytes()).hexdigest()[:16]+'.bin')
    sdf_final=output/('fields_'+hashlib.sha256(sdf_path.read_bytes()).hexdigest()[:16]+'.bin')
    geometry_path.replace(geometry_final);sdf_path.replace(sdf_final)
    for mesh in scene['meshes']:
        mesh['geometry']['path']=geometry_final.name;mesh['sdf']['path']=sdf_final.name
    pending=manifest.with_name(manifest.name+'.'+token+'.tmp')
    pending.write_text(json.dumps(scene, indent=2, allow_nan=False), encoding='utf8');pending.replace(manifest)
    print(f'Exported {len(scene["meshes"])} meshes, {len(scene["materials"])} materials, {len(scene["lights"])} lights to {manifest}')


if __name__ == '__main__': main()
