use anyhow::Result;
use ckb_tool::ckb_types::{packed::*, prelude::*};
use molecule::prelude::*;
use eth_spv_lib::eth_types::*;
use crate::eth_client::types::generated::{double_node_with_merkle_proof, dags_merkle_roots, basic::{BytesVec, Bytes}};
use serde::{Deserialize, Serialize, Deserializer};
use std::convert::TryFrom;
use hex::FromHex;

pub struct Output {
    pub typescript: Script,
    pub lockscript: Script,
    pub amount: u128,
    pub capacity: u64,
}

pub struct Witness {
    pub cell_dep_index_list: Vec<u8>,
    pub header: Vec<u8>,
    pub merkle_proof: Vec<DoubleNodeWithMerkleProof>,
}

pub struct TestCase {
    pub input_capacity: u64,
    pub output_capacity: u64,
    pub input_data: molecule::bytes::Bytes,
    pub output_data: molecule::bytes::Bytes,
    pub witness: Witness,
    pub cell_deps_data: RootsCollectionRaw,
    pub expect_return_code: i8,
}

#[derive(Debug)]
pub struct Hex(pub Vec<u8>);

impl<'de> Deserialize<'de> for Hex {
    fn deserialize<D>(deserializer: D) -> Result<Self, <D as Deserializer<'de>>::Error>
        where
            D: Deserializer<'de>,
    {
        let mut s = <String as Deserialize>::deserialize(deserializer)?;
        if s.starts_with("0x") {
            s = s[2..].to_string();
        }
        if s.len() % 2 == 1 {
            s.insert_str(0, "0");
        }
        Ok(Hex(Vec::from_hex(&s).map_err(|err| {
            serde::de::Error::custom(err.to_string())
        })?))
    }
}

#[derive(Debug, Deserialize)]
pub struct RootsCollectionRaw {
    pub dag_merkle_roots: Vec<Hex>, // H128
}

#[derive(Serialize, Deserialize, Default)]
pub struct RootsCollectionJson {
    pub dag_merkle_roots: Vec<String>,
}

impl TryFrom<RootsCollectionJson> for dags_merkle_roots::DagsMerkleRoots {
    type Error = anyhow::Error;
    fn try_from(roots: RootsCollectionJson) -> Result<Self> {
        let mut roots_vec: Vec<Bytes> = vec![];
        for i in 0..roots.dag_merkle_roots.len() {
            roots_vec.push(hex::decode(&roots.dag_merkle_roots[i])?.into());
        }
        Ok(dags_merkle_roots::DagsMerkleRoots::new_builder()
            .dags_merkle_roots(BytesVec::new_builder().set(roots_vec).build())
            .build())
    }
}

#[derive(Debug)]
pub struct RootsCollection {
    pub dag_merkle_roots: Vec<H128>,
}

impl From<RootsCollectionRaw> for RootsCollection {
    fn from(item: RootsCollectionRaw) -> Self {
        Self {
            dag_merkle_roots: item
                .dag_merkle_roots
                .iter()
                .map(|e| H128::from(&e.0))
                .collect(),
        }
    }
}

pub fn read_roots_collection() -> RootsCollection {
    read_roots_collection_raw().into()
}

pub fn read_roots_collection_raw() -> RootsCollectionRaw {
    serde_json::from_reader(
        std::fs::File::open(std::path::Path::new("../tests/src/eth_client/tests/data/dag_merkle_roots.json")).unwrap(),
    )
        .unwrap()
}

#[derive(Debug, Deserialize)]
struct BlockWithProofsRaw {
    pub proof_length: u64,
    pub header_rlp: Hex,
    pub merkle_root: Hex,        // H128
    pub elements: Vec<Hex>,      // H256
    pub merkle_proofs: Vec<Hex>, // H128
}

pub struct BlockWithProofs {
    pub proof_length: u64,
    pub header_rlp: Hex,
    pub merkle_root: H128,
    pub elements: Vec<H256>,
    pub merkle_proofs: Vec<H128>,
}

impl From<BlockWithProofsRaw> for BlockWithProofs {
    fn from(item: BlockWithProofsRaw) -> Self {
        Self {
            proof_length: item.proof_length,
            header_rlp: item.header_rlp,
            merkle_root: H128::from(&item.merkle_root.0),
            elements: item.elements.iter().map(|e| H256::from(&e.0)).collect(),
            merkle_proofs: item
                .merkle_proofs
                .iter()
                .map(|e| H128::from(&e.0))
                .collect(),
        }
    }
}

pub fn read_block(filename: String) -> BlockWithProofs {
    read_block_raw(filename).into()
}

fn read_block_raw(filename: String) -> BlockWithProofsRaw {
    serde_json::from_reader(std::fs::File::open(std::path::Path::new(&filename)).unwrap()).unwrap()
}

impl BlockWithProofs {
    fn combine_dag_h256_to_h512(elements: Vec<H256>) -> Vec<H512> {
        elements
            .iter()
            .zip(elements.iter().skip(1))
            .enumerate()
            .filter(|(i, _)| i % 2 == 0)
            .map(|(_, (a, b))| {
                let mut buffer = [0u8; 64];
                buffer[..32].copy_from_slice(&(a.0).0);
                buffer[32..].copy_from_slice(&(b.0).0);
                H512(buffer.into())
            })
            .collect()
    }

    pub fn to_double_node_with_merkle_proof_vec(&self) -> Vec<DoubleNodeWithMerkleProof> {
        let h512s = Self::combine_dag_h256_to_h512(self.elements.clone());
        h512s
            .iter()
            .zip(h512s.iter().skip(1))
            .enumerate()
            .filter(|(i, _)| i % 2 == 0)
            .map(|(i, (a, b))| DoubleNodeWithMerkleProof {
                dag_nodes: vec![*a, *b],
                proof: self.merkle_proofs
                    [i / 2 * self.proof_length as usize..(i / 2 + 1) * self.proof_length as usize]
                    .to_vec(),
            })
            .collect()
    }
}

#[derive(Debug)]
pub struct DoubleNodeWithMerkleProof {
    pub dag_nodes: Vec<H512>, // [H512; 2]
    pub proof: Vec<H128>,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct DoubleNodeWithMerkleProofJson {
    pub dag_nodes: Vec<String>, // [H512; 2]
    pub proof: Vec<String>,
}

impl TryFrom<DoubleNodeWithMerkleProofJson> for double_node_with_merkle_proof::DoubleNodeWithMerkleProof {
    type Error = anyhow::Error;
    fn try_from(proof: DoubleNodeWithMerkleProofJson) -> Result<Self> {
        let mut dag_nodes_vec: Vec<Bytes> = vec![];
        for i in 0..proof.dag_nodes.len() {
            dag_nodes_vec.push(hex::decode(&proof.dag_nodes[i])?.into());
        }
        let mut proof_vec: Vec<Bytes> = vec![];
        for i in 0..proof.proof.len() {
            proof_vec.push(hex::decode(&proof.proof[i])?.into());
        }
        Ok(double_node_with_merkle_proof::DoubleNodeWithMerkleProof::new_builder()
            .dag_nodes(BytesVec::new_builder().set(dag_nodes_vec).build())
            .proof(BytesVec::new_builder().set(proof_vec).build())
            .build())
    }
}

impl DoubleNodeWithMerkleProof {
    pub fn new(f:Vec<H512>, s: Vec<H128>) -> Self {
        Self {
            dag_nodes: f,
            proof: s,
        }
    }
    fn truncate_to_h128(arr: H256) -> H128 {
        let mut data = [0u8; 16];
        data.copy_from_slice(&(arr.0).0[16..]);
        H128(data.into())
    }

    fn hash_h128(l: H128, r: H128) -> H128 {
        let mut data = [0u8; 64];
        data[16..32].copy_from_slice(&(l.0).0);
        data[48..64].copy_from_slice(&(r.0).0);
        Self::truncate_to_h128(hash256(&data).into())
    }

    pub fn apply_merkle_proof(&self, index: u64) -> H128 {
        let mut data = [0u8; 128];
        data[..64].copy_from_slice(&(self.dag_nodes[0].0).0);
        data[64..].copy_from_slice(&(self.dag_nodes[1].0).0);

        let mut leaf = Self::truncate_to_h128(hash256(&data).into());

        for i in 0..self.proof.len() {
            if (index >> i as u64) % 2 == 0 {
                leaf = Self::hash_h128(leaf, self.proof[i]);
            } else {
                leaf = Self::hash_h128(self.proof[i], leaf);
            }
        }
        leaf
    }
}