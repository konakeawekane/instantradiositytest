import bpy, bmesh, json

report = {"blender": bpy.app.version_string, "camera": bpy.context.scene.camera.name if bpy.context.scene.camera else None, "objects": [], "materials": []}
dg = bpy.context.evaluated_depsgraph_get()
for obj in bpy.context.scene.objects:
    item = {"name": obj.name, "type": obj.type, "scale": list(obj.scale), "location": list(obj.location)}
    if obj.type == "MESH":
        mesh = obj.evaluated_get(dg).to_mesh()
        bm = bmesh.new(); bm.from_mesh(mesh)
        item.update(vertices=len(mesh.vertices), polygons=len(mesh.polygons), nonmanifold_edges=sum(not e.is_manifold for e in bm.edges), materials=[m.name if m else None for m in mesh.materials], uv_layers=list(mesh.uv_layers.keys()))
        bm.free(); obj.evaluated_get(dg).to_mesh_clear()
    elif obj.type == "LIGHT":
        item.update(light_type=obj.data.type, energy=obj.data.energy, color=list(obj.data.color))
    report["objects"].append(item)
for mat in bpy.data.materials:
    report["materials"].append({"name": mat.name, "nodes": [{"name": n.name, "type": n.type, "image": n.image.filepath if n.type == "TEX_IMAGE" and n.image else None, "inputs": {s.name: [(l.from_node.name, l.from_socket.name) for l in s.links] for s in n.inputs if s.is_linked}} for n in mat.node_tree.nodes] if mat.use_nodes else []})
print(json.dumps(report, indent=2))
