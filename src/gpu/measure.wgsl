struct Params {
    width: u32,
    stride: u32,
    format: u32,
    sample_width: u32,
    sample_height: u32,
}

struct Source {
    params: Params,
    packed: array<u32>,
}

@group(0) @binding(0) var<storage, read> source: Source;
@group(0) @binding(1) var<storage, read_write> samples: array<u32>;

fn byte_at(offset: u32) -> u32 {
    return (source.packed[offset / 4u] >> (8u * (offset % 4u))) & 255u;
}

@compute @workgroup_size(16, 16)
fn measure(@builtin(global_invocation_id) cell: vec3<u32>) {
    let params = source.params;
    if cell.x >= params.sample_width || cell.y >= params.sample_height {
        return;
    }
    let x = ((2u * cell.x + 1u) * params.width) / (2u * params.sample_width);
    let bytes_per_pixel = select(params.format, 4u, params.format == 5u);
    let address = cell.y * params.stride + x * bytes_per_pixel;
    let first = byte_at(address);
    var rgb = first * 65793u;
    if params.format != 1u {
        let middle = byte_at(address + 1u);
        let third = byte_at(address + 2u);
        let red = select(first, third, params.format == 5u);
        let blue = select(third, first, params.format == 5u);
        rgb = red | (middle << 8u) | (blue << 16u);
    }
    samples[cell.y * params.sample_width + cell.x] = rgb;
}
