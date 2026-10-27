// Loads exported files with Apple's Model I/O (the USD/OBJ/STL/PLY loader behind Quick Look
// and AR) and reports what it finds. Usage: swift verify_modelio.swift <file>...
import Foundation
import ModelIO

for path in CommandLine.arguments.dropFirst() {
    let url = URL(fileURLWithPath: path)
    guard MDLAsset.canImportFileExtension(url.pathExtension) else {
        print("VERIFY \(url.lastPathComponent): extension not supported")
        continue
    }
    let asset = MDLAsset(url: url)
    var meshes = 0, vertices = 0, triangles = 0, withUV = 0, materials = Set<String>()
    func walk(_ o: MDLObject) {
        if let m = o as? MDLMesh {
            meshes += 1
            vertices += m.vertexCount
            if m.vertexAttributeData(forAttributeNamed: MDLVertexAttributeTextureCoordinate) != nil { withUV += 1 }
            for case let sub as MDLSubmesh in m.submeshes ?? [] {
                triangles += sub.indexCount / 3
                if let mat = sub.material { materials.insert(mat.name) }
            }
        }
        for child in o.children.objects { walk(child) }
    }
    for i in 0..<asset.count { walk(asset.object(at: i)) }
    print("VERIFY \(url.lastPathComponent): meshes=\(meshes) vertices=\(vertices) triangles=\(triangles) with_uv=\(withUV) materials=\(materials.count)")
}
