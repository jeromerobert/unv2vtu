use clap::Parser;
use rustc_hash::FxHashMap;
use std::{
    fs::File,
    io::{self, BufRead, BufReader, BufWriter},
    path::{Path, PathBuf},
};
use tucanos_vtkio::UnstructuredGridWriter;

#[derive(Debug, Default)]
pub struct UnvFile {
    pub nodes: Vec<Node>,
    // TODO: avoid this Vec in Vec (too many malloc)
    pub elements: Vec<Element>,
    pub groups: Vec<Group>,
}

#[derive(Debug)]
pub struct Node {
    pub label: usize,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(Debug)]
pub struct Element {
    pub label: usize,
    pub descriptor: usize,
    pub nodes: Vec<usize>,
}

#[derive(Debug)]
pub struct Group {
    pub id: usize,
    pub name: String,
    pub entities: Vec<GroupEntity>,
}

#[derive(Debug)]
pub struct GroupEntity {
    pub entity_type: usize, // e.g., 7 for Node, 8 for Element
    pub entity_tag: usize,  // The label of the node or element
}

impl UnvFile {
    pub fn parse_file<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        Self::parse(reader)
    }

    pub fn parse<R: BufRead>(reader: R) -> io::Result<Self> {
        let mut unv = Self::default();
        let mut lines = reader.lines().map_while(Result::ok);
        while let Some(line) = lines.next() {
            if line.trim() == "-1"
                && let Some(dataset_id_line) = lines.next()
            {
                let dataset_id: i32 = dataset_id_line.trim().parse().unwrap_or(0);

                // Read all lines until the next "-1"
                let mut dataset_lines = Vec::new();
                for next_line in &mut lines {
                    if next_line.trim() == "-1" {
                        break;
                    }
                    dataset_lines.push(next_line);
                }

                match dataset_id {
                    2411 => unv.parse_nodes_2411(&dataset_lines),
                    2412 => unv.parse_elements_2412(&dataset_lines),
                    2467 | 2435 => unv.parse_groups_2467(&dataset_lines),
                    _ => { /* Ignore unsupported datasets */ }
                }
            }
        }
        Ok(unv)
    }

    fn parse_nodes_2411(&mut self, lines: &[String]) {
        let mut i = 0;
        while i < lines.len() {
            let meta_tokens: Vec<&str> = lines[i].split_whitespace().collect();
            if meta_tokens.is_empty() || i + 1 >= lines.len() {
                break;
            }

            let label: usize = meta_tokens[0].parse().unwrap_or(0);
            // TODO: move allocation outside of loop
            let coords: Vec<_> = lines[i + 1].split_whitespace().collect();
            if coords.len() >= 3 {
                self.nodes.push(Node {
                    label,
                    x: coords[0].parse().unwrap_or(0.0),
                    y: coords[1].parse().unwrap_or(0.0),
                    z: coords[2].parse().unwrap_or(0.0),
                });
            }
            i += 2;
        }
    }

    fn parse_elements_2412(&mut self, lines: &[String]) {
        let mut i = 0;
        while i < lines.len() {
            // TODO: move allocation outside of loop
            let meta: Vec<_> = lines[i].split_whitespace().collect();
            if meta.len() < 6 || i + 1 >= lines.len() {
                break;
            }

            let label: usize = meta[0].parse().unwrap_or(0);
            let descriptor: usize = meta[1].parse().unwrap_or(0);
            let num_nodes: usize = meta[5].parse().unwrap_or(0);

            let mut j = i + 1;

            // Beam, rod, and pipe descriptors (11, 21-24, 31-32) contain an extra
            // orientation/cross-section record (Record 2) right after the header.
            if matches!(descriptor, 11 | 21..=24 | 31..=32) {
                j += 1;
            }

            let mut node_labels = Vec::new();
            let mut nodes_collected = 0;

            while nodes_collected < num_nodes && j < lines.len() {
                let node_line: Vec<&str> = lines[j].split_whitespace().collect();
                for n in node_line {
                    if nodes_collected >= num_nodes {
                        break;
                    }
                    if let Ok(val) = n.parse::<usize>() {
                        node_labels.push(val);
                        nodes_collected += 1;
                    }
                }
                j += 1;
            }

            self.elements.push(Element {
                label,
                descriptor,
                nodes: node_labels,
            });
            i = j;
        }
    }

    fn parse_groups_2467(&mut self, lines: &[String]) {
        let mut i = 0;
        while i < lines.len() {
            let meta: Vec<_> = lines[i].split_whitespace().collect();
            if meta.is_empty() || i + 2 >= lines.len() {
                break;
            }

            let id: usize = meta[0].parse().unwrap_or(0);
            let num_entities: usize = meta.last().unwrap_or(&"0").parse().unwrap_or(0);

            let name = lines[i + 1].trim().to_string();
            // TODO: with_capacity ?
            let mut entities = Vec::new();

            let mut j = i + 2;
            let mut entities_collected = 0;

            while entities_collected < num_entities && j < lines.len() {
                // TODO: move allocation outside of loop
                let entity_tokens: Vec<_> = lines[j].split_whitespace().collect();

                // Entity definitions are in pairs: [Type, Tag, Type, Tag, ...]
                let mut k = 0;
                while k + 1 < entity_tokens.len() && entities_collected < num_entities {
                    let entity_type = entity_tokens[k].parse().unwrap_or(0);
                    let entity_tag = entity_tokens[k + 1].parse().unwrap_or(0);
                    let _leaf_id = entity_tokens
                        .get(k + 2)
                        .map_or(0, |v| v.parse().unwrap_or(0));
                    let _component_id = entity_tokens
                        .get(k + 3)
                        .map_or(0, |v| v.parse().unwrap_or(0));

                    // Basic handling: only capturing Type and Tag for simplicity
                    entities.push(GroupEntity {
                        entity_type,
                        entity_tag,
                    });

                    entities_collected += 1;
                    // UNV entity records take 4 fields per item (Type, Tag, Leaf, Component)
                    // If formatting differs, adjust step size. Using step of 2 for simplicity
                    // if Leaf/Component are missing, but standard is 4 fields.
                    k += if entity_tokens.len().is_multiple_of(4) {
                        4
                    } else {
                        2
                    };
                }
                j += 1;
            }

            self.groups.push(Group { id, name, entities });
            i = j;
        }
    }

    /// Exports the UNV mesh structure to a VTU file at the specified output path.
    ///
    /// # Arguments
    ///
    /// * `path` - Path where the resulting VTU file will be written.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Result`] if opening the destination path or writing
    /// file headers/payload fails.
    pub fn export_vtu<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        // Create a mapping from UNV Node Label -> Zero-based index
        let mut node_map = FxHashMap::default();
        for (idx, node) in self.nodes.iter().enumerate() {
            node_map.insert(node.label, idx);
        }

        // TODO: and element or a node can be in multiple groups. use a Vec<FxHashMap>.
        // Map groups to elements (entity_type 8) and nodes (entity_type 7)
        let mut elem_to_group = FxHashMap::default();
        let mut node_to_group = FxHashMap::default();

        for group in &self.groups {
            for entity in &group.entities {
                match entity.entity_type {
                    7 => {
                        node_to_group.insert(entity.entity_tag, group.id);
                    } // Node Group
                    8 => {
                        elem_to_group.insert(entity.entity_tag, group.id);
                    } // Element Group
                    _ => {}
                }
            }
        }

        // Prepare Point Data (Node Groups & Original UNV Node Labels)
        let mut point_group_ids = Vec::with_capacity(self.nodes.len());
        let mut global_node_ids = Vec::with_capacity(self.nodes.len());
        for node in &self.nodes {
            point_group_ids.push(node_to_group.get(&node.label).copied().unwrap_or(0));
            global_node_ids.push(node.label);
        }

        // Prepare Cell Data & Connectivity
        let mut connectivity = Vec::new();
        let mut offsets = Vec::new();
        let mut cell_types = Vec::new();
        let mut cell_group_ids = Vec::new();
        let mut global_element_ids = Vec::new();
        let mut current_offset = 0;

        for element in &self.elements {
            let vtk_type = match element.descriptor {
                11 | 21 => 3,  // Line -> VTK_LINE
                41 | 91 => 5,  // Triangle -> VTK_TRIANGLE
                44 | 94 => 9,  // Quadrangle -> VTK_QUAD
                111 => 10,     // Tetrahedron -> VTK_TETRA
                112 => 13,     // Wedge -> VTK_WEDGE
                115 => 12,     // Hexahedron -> VTK_HEXAHEDRON
                _ => continue, // Skip unsupported elements
            };

            let mut valid_nodes = Vec::new();
            for &node_label in &element.nodes {
                if let Some(&idx) = node_map.get(&node_label) {
                    valid_nodes.push(idx);
                }
            }

            if valid_nodes.len() != element.nodes.len() {
                continue;
            }

            connectivity.extend(valid_nodes);
            current_offset += element.nodes.len();

            offsets.push(current_offset);
            cell_types.push(vtk_type);
            cell_group_ids.push(elem_to_group.get(&element.label).copied().unwrap_or(0));
            global_element_ids.push(element.label);
        }

        let num_points = self.nodes.len();
        let num_cells = cell_types.len();
        let mut vtu_writer = UnstructuredGridWriter::default();
        vtu_writer.set_num_cells(num_cells);
        vtu_writer.set_num_points(num_points);
        // TODO we need GroupIDs2 / GroupNames2, GroupIDs3 / GroupNames3, ... because an element or
        // a node can be in multiple groupes. We also need NodeGroupID2, GroupID2, ...
        // Write FieldData (Group Name & ID Dictionary)
        if !self.groups.is_empty() {
            vtu_writer.add_field_data(
                "GroupIDs",
                self.groups.len(),
                1,
                self.groups.iter().map(|g| g.id),
            );
            vtu_writer.add_field_str("GroupNames", 1, self.groups.iter().map(|g| g.name.as_str()));
        }
        vtu_writer.add_point_data("NodeGroupID", 1, point_group_ids);
        vtu_writer.add_point_data("GlobalNodeID", 1, global_node_ids);
        vtu_writer.add_points(self.nodes.iter().flat_map(|n| [n.x, n.y, n.z]));
        vtu_writer.add_cells(connectivity.len(), connectivity, offsets, cell_types);
        vtu_writer.add_cell_data("GroupID", 1, cell_group_ids);
        vtu_writer.add_cell_data("GlobalElementID", 1, global_element_ids);
        vtu_writer.write(&mut BufWriter::new(File::create(path)?))?;
        Ok(())
    }
}

#[derive(Parser, Debug)]
#[command(author, version, about = "Converts UNV mesh files into VTU format.", long_about = None)]
struct Args {
    /// Path to the input .unv file
    #[arg(value_name = "INPUT")]
    input: PathBuf,

    /// Path to the output .vtu file
    #[arg(value_name = "OUTPUT")]
    output: PathBuf,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Parse the UNV file
    let unv = UnvFile::parse_file(&args.input)?;

    // Export directly to VTU
    unv.export_vtu(&args.output)?;

    println!(
        "Successfully converted {} to {} with {} nodes and {} elements.",
        args.input.display(),
        args.output.display(),
        unv.nodes.len(),
        unv.elements.len()
    );

    Ok(())
}
