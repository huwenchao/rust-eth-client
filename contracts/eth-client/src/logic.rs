use crate::types::{Error, CellDataView, witness::WitnessReader, basic::{ChainReader, Uint64}, dags_merkle_roots::DagsMerkleRootsReader, double_node_with_merkle_proof::DoubleNodeWithMerkleProofReader};
use crate::helper::{*, DoubleNodeWithMerkleProof};
use alloc::{vec, vec::Vec};
use ckb_std::{
    ckb_constants::Source,
    debug,
    high_level::{load_cell_data, load_witness_args, QueryIter},
};
use molecule::prelude::{Reader,  Entity};
use eth_spv_lib::eth_types::*;
use crate::types::basic::{ HeaderInfoReader, BytesVecReader};

pub const MAIN_HEADER_CACHE_LIMIT: usize = 500;
pub const UNCLE_HEADER_CACHE_LIMIT: usize = 500;

#[derive(Debug)]
pub struct CellDataTuple(Option<CellDataView>, Option<CellDataView>);

pub fn verify() -> Result<(), Error> {
    let input_data = get_data(Source::GroupInput)?.expect("should not happen");
    let output_data = get_data(Source::GroupOutput)?.expect("should not happen");
    verify_data(&input_data, &output_data)?;
    debug!("verify data finish");
    verify_witness(&input_data, &output_data)?;
    Ok(())
}

fn verify_data(
    input_data: &CellDataView,
    output_data: &CellDataView,
) -> Result<(), Error> {
    if input_data.user_lockscript.as_ref() != output_data.user_lockscript.as_ref()
    {
        return Err(Error::InvalidDataChange);
    }
    Ok(())
}

/// ensure transfer happen on XChain by verifying the spv proof
fn verify_witness(input: &CellDataView, output: &CellDataView) -> Result<(), Error> {
    let witness_args = load_witness_args(0, Source::GroupInput)?.input_type();
    if witness_args.is_none() {
        return Err(Error::InvalidWitness);
    }
    let witness_args = witness_args.to_opt().unwrap().raw_data();
    if WitnessReader::verify(&witness_args, false).is_err() {
        return Err(Error::InvalidWitness);
    }
    let witness = WitnessReader::new_unchecked(&witness_args);
    // parse header
    let header_raw = witness.header().raw_data();
    // check input && output data
    let header = verify_input_output_data(input, output, header_raw)?;
    // parse merkle proof
    let mut proofs = vec![];
    for i in 0..witness.merkle_proof().len() {
        let proof_raw = witness.merkle_proof().get_unchecked(i).raw_data();
        let proof = parse_proof(proof_raw)?;
        proofs.push(proof);
    }
    // parse dep data
    let merkle_root = parse_dep_data(witness, header.number)?;
    if !verify_header(&header, Option::None, merkle_root, &proofs) {
        return Err(Error::InvalidMerkleProofData);
    }
    Ok(())
}

fn verify_input_output_data(input: &CellDataView, output: &CellDataView, header_raw: &[u8]) -> Result<BlockHeader, Error> {
    debug!("verify input && output data. make sure the main chain is right.");
    let header: BlockHeader = rlp::decode(header_raw.to_vec().as_slice()).unwrap();
    debug!("header after decode is {:?}", header);

    if ChainReader::verify(&input.headers, false).is_err() {
        return Err(Error::InvalidCellData);
    }
    let chain_input_reader = ChainReader::new_unchecked(&input.headers);
    let main_input_reader = chain_input_reader.main();
    debug!("input: the main chain length: {:?}", main_input_reader.len());
    let uncle_input_reader = chain_input_reader.uncle();
    if ChainReader::verify(&output.headers, false).is_err() {
        return Err(Error::InvalidCellData);
    }
    let chain_output_reader = ChainReader::new_unchecked(&output.headers);
    let main_output_reader = chain_output_reader.main();
    let uncle_output_reader = chain_output_reader.uncle();
    debug!("output: the main chain length: {:?}", main_output_reader.len());
    // header is on main chain.
    let main_tail_info_input = main_input_reader.get_unchecked(main_input_reader.len() - 1).raw_data();
    if HeaderInfoReader::verify(&main_tail_info_input, false).is_err() {
        return Err(Error::InvalidCellData);
    }
    let main_tail_info_input_reader = HeaderInfoReader::new_unchecked(main_tail_info_input);
    let main_tail_header_input = main_tail_info_input_reader.header().raw_data();

    let main_tail_info_output = main_output_reader.get_unchecked(main_output_reader.len() - 1).raw_data();
    if HeaderInfoReader::verify(&main_tail_info_output, false).is_err() {
        return Err(Error::InvalidCellData);
    }
    let main_tail_info_output_reader = HeaderInfoReader::new_unchecked(main_tail_info_output);
    let main_tail_header_output = main_tail_info_output_reader.header().raw_data();

    // header is on main chain.
    if main_tail_header_output == header_raw {
        debug!("the new header is on main chain");
        assert_eq!(main_tail_info_output_reader.hash().raw_data(), header.hash.unwrap().0.as_bytes());
        let main_tail_input: BlockHeader = rlp::decode(main_tail_header_input.to_vec().as_slice()).unwrap();
        debug!("new header parent hash: {:?} ", header.parent_hash.0);
        debug!("input main chain tail hash: {:?}", main_tail_input.hash.unwrap().0);
        // if header.parent_hash == tail_input.hash => the chain is not reorg.
        // else do reorg.
        if main_tail_input.hash.unwrap() == header.parent_hash {
            debug!("the main chain is not reorg.");
            let prev_difficult: Uint64 = main_tail_info_input_reader.total_difficulty().to_entity();
            let left: Uint64 = main_tail_info_output_reader.total_difficulty().to_entity();
            let right: Uint64 = header.difficulty.0.as_u64().into();
            debug!("The total difficulty of the output chain is the total difficulty of the input chain plus the difficulty of the new block");
            debug!("left difficulty u64: {} right difficulty u64: {}", to_u64(&left), to_u64(&right).checked_add(to_u64(&prev_difficult)).unwrap());
            assert_eq!(to_u64(&left), to_u64(&right).checked_add(to_u64(&prev_difficult)).unwrap());

            if main_output_reader.len() > MAIN_HEADER_CACHE_LIMIT {
                return Err(Error::InvalidCellData);
            }
            debug!("the uncle chain should be the same");
            verify_original_chain_data(main_input_reader, main_output_reader, MAIN_HEADER_CACHE_LIMIT)?;
            // the uncle chain should be the same.
            assert_eq!(uncle_input_reader.as_slice(),uncle_output_reader.as_slice());
        } else {
            debug!("warning: the main chain had been reorged.");
            let left: Uint64 = main_tail_info_input_reader.total_difficulty().to_entity();
            let right: Uint64 = main_tail_info_output_reader.total_difficulty().to_entity();
            if to_u64(&right) >= to_u64(&left) {// header.number < main_tail_input.number
                // assert_eq!(main_tail_input.number - header.number > 0, true)
                let mut number = header.number - 1;
                let mut current_hash = header.parent_hash;
                loop {
                    if number == 0 {
                        return Err(Error::InvalidCellData);
                    }
                    // find parent header.
                    if main_tail_input.number <= number { // the parent header is on uncle chain.
                        traverse_uncle_chain(uncle_input_reader, &mut current_hash, &mut number)?;
                    } else {
                        let offset = (main_tail_input.number - number-1) as usize;
                        if offset > main_input_reader.len() {
                            return Err(Error::InvalidCellData);
                        }
                        let header_info_temp = main_input_reader.get_unchecked(main_input_reader.len()-1-offset).raw_data();
                        if HeaderInfoReader::verify(&header_info_temp, false).is_err() {
                            return Err(Error::InvalidCellData);
                        }
                        let header_info_temp_reader = HeaderInfoReader::new_unchecked(header_info_temp);
                        // let header_temp: BlockHeader = rlp::decode(header_temp.to_vec().as_slice()).unwrap();
                        if header_info_temp_reader.hash().raw_data() == current_hash.0.as_bytes() {// the parent header is on main chain.
                            let mut input_data = vec![];
                            for i in 1..main_input_reader.len()-1-offset {
                                input_data.push(main_input_reader.get_unchecked(i).raw_data())
                            }
                            let mut output_data = vec![];
                            for i in 0..main_output_reader.len()-1 {
                                output_data.push(main_output_reader.get_unchecked(i).raw_data())
                            }
                            assert_eq!(input_data, output_data);
                            break;
                        } else {// the parent header is on uncle chain.
                            traverse_uncle_chain(uncle_input_reader, &mut current_hash, &mut number)?;
                        }
                    }
                }
            } else {
                return Err(Error::InvalidCellData);
            }
        }
    } else {
        debug!("warning: the new header is not on main chain.");
        // the header is on uncle chain. just do append.
        verify_original_chain_data(uncle_input_reader, uncle_output_reader, UNCLE_HEADER_CACHE_LIMIT)?;
        // the main chain should be the same.
        assert_eq!(main_output_reader.as_slice(),main_input_reader.as_slice());
    }
    // assert_eq!(main_output_reader.get_unchecked(main_output_reader.len() - 1).raw_data(), header_raw);
    Ok(header)
}

fn traverse_uncle_chain(uncle_input_reader: BytesVecReader,  current_hash: &mut H256,  number: &mut u64) -> Result<(), Error>{
    let mut index = uncle_input_reader.len()-1;
    loop {
        if index == 0 {
            return Err(Error::InvalidCellData);
        }
        let uncle_tail_input = uncle_input_reader.get_unchecked(index).raw_data();
        if HeaderInfoReader::verify(&uncle_tail_input, false).is_err() {
            return Err(Error::InvalidCellData);
        }
        let uncle_header_info_reader = HeaderInfoReader::new_unchecked(uncle_tail_input);
        if uncle_header_info_reader.hash().raw_data() == current_hash.0.as_bytes() {
            let uncle_header_raw = uncle_header_info_reader.header().raw_data();
            let uncle_header: BlockHeader = rlp::decode(uncle_header_raw.to_vec().as_slice()).unwrap();
            // TODO: make sure the header on uncle chain also exist on the main chain.

            *number -= 1;
            *current_hash = uncle_header.parent_hash;
            break;
        } else {
            index -= 1;
        }
    }
    Ok(())
}

fn verify_original_chain_data(uncle_input_reader: BytesVecReader, uncle_output_reader: BytesVecReader, limit: usize) -> Result<(), Error> {
    if uncle_input_reader.len() == uncle_output_reader.len() && uncle_output_reader.len() == limit {
        let mut input_data = vec![];
        for i in 1..uncle_input_reader.len() {
            input_data.push(uncle_input_reader.get_unchecked(i).raw_data())
        }
        let mut output_data = vec![];
        for i in 0..uncle_output_reader.len()-1 {
            output_data.push(uncle_output_reader.get_unchecked(i).raw_data())
        }
        assert_eq!(input_data, output_data);
    } else if uncle_input_reader.len() < uncle_output_reader.len(){
        let mut input_data = vec![];
        for i in 0..uncle_input_reader.len() {
            input_data.push(uncle_input_reader.get_unchecked(i).raw_data())
        }
        let mut output_data = vec![];
        for i in 0..uncle_output_reader.len()-1 {
            output_data.push(uncle_output_reader.get_unchecked(i).raw_data())
        }
        assert_eq!(input_data, output_data);
    } else {
        return Err(Error::InvalidCellData);
    }
    Ok(())
}

fn parse_proof(proof_raw: &[u8]) -> Result<DoubleNodeWithMerkleProof, Error> {
    if DoubleNodeWithMerkleProofReader::verify(&proof_raw, false).is_err() {
        return Err(Error::InvalidWitness);
    }
    let merkle_proof = DoubleNodeWithMerkleProofReader::new_unchecked(proof_raw);
    let mut dag_nodes = vec![];
    for i in 0..merkle_proof.dag_nodes().len() {
        let mut node = [0u8; 64];
        node.copy_from_slice(merkle_proof.dag_nodes().get_unchecked(i).raw_data());
        dag_nodes.push(H512(node.into()));
    }
    let mut proofs = vec![];
    for i in 0..merkle_proof.proof().len() {
        let mut proof = [0u8; 16];
        proof.copy_from_slice(merkle_proof.proof().get_unchecked(i).raw_data());
        proofs.push(H128(proof.into()));
    }
    Ok(DoubleNodeWithMerkleProof::new(
        dag_nodes,
        proofs,
    ))
}

fn parse_dep_data(witness: WitnessReader, number: u64) -> Result<H128, Error> {
    let cell_dep_index_list = witness.cell_dep_index_list().raw_data();
    if cell_dep_index_list.len() != 1 {
        return Err(Error::InvalidWitness);
    }
    let dep_data = load_cell_data(cell_dep_index_list[0].into(), Source::CellDep)?;
    // debug!("dep data is {:?}", &dep_data);
    if DagsMerkleRootsReader::verify(&dep_data, false).is_err() {
        return Err(Error::DagsMerkleRootsDataInvalid);
    }
    let dags_reader = DagsMerkleRootsReader::new_unchecked(&dep_data);
    let idx: usize = (number / 30000) as usize;
    let merkle_root_tmp = dags_reader.dags_merkle_roots().get_unchecked(idx).raw_data();
    let mut merkle_root = [0u8; 16];
    merkle_root.copy_from_slice(merkle_root_tmp);
    Ok(H128(merkle_root.into()))
}

fn get_data(source: Source) -> Result<Option<CellDataView>, Error> {
    let data_list = QueryIter::new(load_cell_data, source).collect::<Vec<Vec<u8>>>();
    match data_list.len() {
        0 => Ok(None),
        1 => Ok(Some(CellDataView::from_slice(
            data_list[0].as_slice(),
        )?)),
        _ => Err(Error::TxInvalid),
    }
}

fn to_u64(data: &Uint64) -> u64 {
    let mut res = [0u8; 8];
    res.copy_from_slice(data.as_slice());
    u64::from_be_bytes(res)
}
