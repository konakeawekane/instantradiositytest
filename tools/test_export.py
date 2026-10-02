"""Run with Blender --background --python tools/test_export.py.
Creates fixtures in artifacts/export_tests and tests the real export pipeline.
"""
import array
import importlib.util
import json
import math
from pathlib import Path
import struct
import sys
import unittest
import bpy
from mathutils import Vector

module = importlib.util.spec_from_file_location('export_scene', Path(__file__).with_name('export_scene.py'))
exporter = importlib.util.module_from_spec(module); module.loader.exec_module(exporter)
ROOT = Path(__file__).resolve().parents[1]


class ExportTests(unittest.TestCase):
    def setUp(self):
        bpy.ops.wm.read_factory_settings(use_empty=True)
        bpy.ops.mesh.primitive_cube_add()
        self.cube=bpy.context.object; self.cube.name='textured_cube'
        self.mat=bpy.data.materials.new('texture_material'); self.mat.use_nodes=True
        self.bsdf=self.mat.node_tree.nodes.get('Principled BSDF')
        self.bsdf.inputs['Emission Strength'].default_value=0
        self.cube.data.materials.append(self.mat)
        bpy.ops.object.camera_add(location=(0,0,5))
        bpy.context.scene.camera=bpy.context.object
        bpy.ops.object.light_add(type='POINT', location=(0,0,3))
        bpy.context.object.data.energy=10

    def image(self,name,color,non_color=False):
        image=bpy.data.images.new(name,8,8,alpha=True)
        if non_color:image.colorspace_settings.name='Non-Color'
        image.pixels[:]=list(color)*64
        return image

    def export(self,folder):
        path=ROOT/'artifacts'/'export_tests'/folder/'scene.json'
        sys.argv=['export_scene.py','--','--output',str(path),'--sdf-resolution','16']
        exporter.main()
        return path,json.loads(path.read_text())

    def test_textures_constants_baked_scale_and_distinct_resolutions(self):
        tree=self.mat.node_tree
        base=tree.nodes.new('ShaderNodeTexImage');base.image=self.image('base',(.5,.25,.75,1))
        tree.links.new(base.outputs['Color'],self.bsdf.inputs['Base Color'])
        packed=tree.nodes.new('ShaderNodeTexImage');packed.image=self.image('packed',(.8,.5,.125,1),True)
        split=tree.nodes.new('ShaderNodeSeparateColor');tree.links.new(packed.outputs['Color'],split.inputs['Color'])
        tree.links.new(split.outputs['Green'],self.bsdf.inputs['Roughness']);tree.links.new(split.outputs['Blue'],self.bsdf.inputs['Metallic'])
        normals=tree.nodes.new('ShaderNodeTexImage');normals.image=self.image('normal',(.5,.5,1,1),True)
        normalmap=tree.nodes.new('ShaderNodeNormalMap');tree.links.new(normals.outputs['Color'],normalmap.inputs['Color']);tree.links.new(normalmap.outputs['Normal'],self.bsdf.inputs['Normal'])
        self.cube.scale=(2,1,.5);self.cube['sdf_resolution']=16
        constant=self.cube.copy();constant.data=self.cube.data.copy();constant.data.materials.clear()
        plain=bpy.data.materials.new('constant');plain.diffuse_color=(.2,.4,.6,1);constant.data.materials.append(plain)
        constant.location=(4,0,0);constant.scale=(1,1,1);constant['sdf_resolution']=24;bpy.context.collection.objects.link(constant)
        path,s=self.export('textures')
        self.assertEqual(2,len(s['meshes']));self.assertEqual({16,24},{m['sdf']['resolution'] for m in s['meshes']})
        obj=next(o for o in s['objects'] if o['id']=='textured_cube');self.assertEqual(1,obj['transform']['scale'])
        mat=next(m for m in s['materials'] if m['id']=='texture_material')
        for name in ['base_color','roughness','metallic','normal']:self.assertTrue((path.parent/mat[name+'_texture']).is_file())
        self.assertEqual([0,1,2,0],mat['texture_channels'])
        self.assertEqual([1,1,1,1],mat['base_color'])
        mesh=next(m for m in s['meshes'] if m['id']==obj['mesh']);g=mesh['geometry'];data=(path.parent/g['path']).read_bytes()
        vertices=[struct.unpack_from('<16f',data,g['vertex_offset']+i*64) for i in range(g['vertex_count'])]
        self.assertAlmostEqual(2,max(v[0] for v in vertices));self.assertAlmostEqual(.5,max(v[2] for v in vertices))
        self.assertEqual((2,1,.5),tuple(self.cube.scale)) # Source object wasn't changed.

    def test_open_mesh_rejected(self):
        bpy.data.objects.remove(self.cube,do_unlink=True)
        bpy.ops.mesh.primitive_plane_add()
        with self.assertRaisesRegex(ValueError,'watertight'):self.export('open_rejected')

    def test_alpha_scalar_and_unsupported_color_links(self):
        tree=self.mat.node_tree
        image=tree.nodes.new('ShaderNodeTexImage');image.image=self.image('alpha_scalar',(.1,.2,.3,.375),True)
        tree.links.new(image.outputs['Alpha'],self.bsdf.inputs['Roughness'])
        _,scene=self.export('alpha_scalar')
        self.assertEqual(3,scene['materials'][0]['texture_channels'][1])
        tree.links.new(image.outputs['Alpha'],self.bsdf.inputs['Base Color'])
        with self.assertRaisesRegex(ValueError,'image Color output'):self.export('alpha_color_rejected')
        tree.links.remove(self.bsdf.inputs['Base Color'].links[0])
        split=tree.nodes.new('ShaderNodeSeparateColor');tree.links.new(image.outputs['Color'],split.inputs['Color'])
        tree.links.new(split.outputs['Red'],self.bsdf.inputs['Base Color'])
        with self.assertRaisesRegex(ValueError,'full image Color output'):self.export('split_color_rejected')

    def test_scalar_image_color_must_be_grayscale_or_separated(self):
        tree=self.mat.node_tree
        node=tree.nodes.new('ShaderNodeTexImage');node.image=self.image('rgb_scalar',(.1,.2,.3,1),True)
        tree.links.new(node.outputs['Color'],self.bsdf.inputs['Roughness'])
        with self.assertRaisesRegex(ValueError,'must be grayscale'):self.export('rgb_scalar_rejected')
        node.image=self.image('gray_scalar',(.375,.375,.375,1),True)
        _,scene=self.export('gray_scalar')
        self.assertEqual(0,scene['materials'][0]['texture_channels'][1])
        node.interpolation='Closest'
        with self.assertRaisesRegex(ValueError,'interpolation must be Linear'):self.export('interpolation_rejected')

    def test_failed_export_preserves_previous_manifest_and_payloads(self):
        path,s=self.export('atomic')
        files=[path]+[path.parent/m['geometry']['path'] for m in s['meshes']]+[path.parent/m['sdf']['path'] for m in s['meshes']]
        before={p:p.read_bytes() for p in files}
        bpy.ops.mesh.primitive_plane_add()
        with self.assertRaisesRegex(ValueError,'watertight'):self.export('atomic')
        self.assertEqual(before,{p:p.read_bytes() for p in files})

    def test_default_black_emission_is_valid_but_emitting_material_is_rejected(self):
        self.bsdf.inputs['Emission Color'].default_value=(0,0,0,1)
        self.bsdf.inputs['Emission Strength'].default_value=1
        self.export('default_black_emission')
        self.bsdf.inputs['Emission Color'].default_value=(1,0,0,1)
        with self.assertRaisesRegex(ValueError,'unsupported emission'):self.export('emission_rejected')

    def test_texture_requires_active_uv_and_correct_data_color_space(self):
        tree=self.mat.node_tree;image=tree.nodes.new('ShaderNodeTexImage');image.image=self.image('roughness',(.5,.5,.5,1))
        tree.links.new(image.outputs['Color'],self.bsdf.inputs['Roughness'])
        with self.assertRaisesRegex(ValueError,'Non-Color'):self.export('scalar_srgb_rejected')
        image.image.colorspace_settings.name='Non-Color'
        uv=tree.nodes.new('ShaderNodeUVMap');uv.uv_map='missing_uv';tree.links.new(uv.outputs['UV'],image.inputs['Vector'])
        with self.assertRaisesRegex(ValueError,'active UV map'):self.export('uv_rejected')

    def test_checker_texture_fixture_for_linear_gpu_filtering(self):
        # Assign one planar UV projection per cube face with no degenerate UVs.
        layer=self.cube.data.uv_layers.active
        for face in self.cube.data.polygons:
            axes=(0,1) if abs(face.normal.z)>.5 else ((1,2) if abs(face.normal.x)>.5 else (0,2))
            for index in face.loop_indices:
                p=self.cube.data.vertices[self.cube.data.loops[index].vertex_index].co
                layer.data[index].uv=((p[axes[0]]+1)/2,(p[axes[1]]+1)/2)
        image=bpy.data.images.new('checker',8,8)
        image.pixels[:]=[v for y in range(8) for x in range(8) for v in (((x+y)%2,)*3+(1,))]
        node=self.mat.node_tree.nodes.new('ShaderNodeTexImage');node.image=image
        self.mat.node_tree.links.new(node.outputs['Color'],self.bsdf.inputs['Base Color'])
        path,s=self.export('filtering');self.assertTrue(s['materials'][0]['base_color_srgb'])
        self.assertTrue((path.parent/s['materials'][0]['base_color_texture']).is_file())

    def test_sdf_signed_samples_match_known_box(self):
        path,s=self.export('cube');mesh=s['meshes'][0];f=mesh['sdf'];n=f['resolution']
        values=struct.unpack_from('<'+str(n**3)+'f',(path.parent/f['path']).read_bytes(),f['offset'])
        for z in range(n):
            for y in range(n):
                for x in range(n):
                    p=Vector(tuple(f['bounds_min'][a]+(f['bounds_max'][a]-f['bounds_min'][a])*c/(n-1) for a,c in enumerate((x,y,z))))
                    q=Vector(tuple(abs(v)-1 for v in p));outside=Vector(tuple(max(v,0) for v in q)).length;exact=outside+min(max(q),0)
                    self.assertAlmostEqual(exact-f['error_bound'],values[x+n*(y+n*z)],delta=2e-5)
        self.assertLess(min(values),0);self.assertGreater(max(values),0)


if __name__=='__main__':
    suite=unittest.defaultTestLoader.loadTestsFromTestCase(ExportTests)
    result=unittest.TextTestRunner(verbosity=2).run(suite)
    if not result.wasSuccessful():raise RuntimeError('Exporter tests failed')
