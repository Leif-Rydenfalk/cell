use cell_sdk::rkyv;
use cell_sdk::rkyv::Deserialize; // <--- Added this import
use cell_sdk::vesicle::Vesicle;

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, PartialEq)]
#[archive(check_bytes)]
struct TestDto {
    id: u32,
    name: String,
    data: Vec<u8>,
}

#[test]
fn test_vesicle_serialization_round_trip() {
    let original = TestDto {
        id: 42,
        name: "Cell".to_string(),
        data: vec![0xDE, 0xAD, 0xBE, 0xEF],
    };

    // 1. Serialize
    let bytes = rkyv::to_bytes::<_, 256>(&original)
        .expect("Failed to serialize")
        .into_vec();

    // 2. Wrap in Vesicle
    let v = Vesicle::wrap(bytes);

    // 3. Access / Verify (Zero Copy)
    let archived = rkyv::check_archived_root::<TestDto>(v.as_slice()).expect("Failed to verify");
    assert_eq!(archived.id, 42);
    assert_eq!(archived.name, "Cell");

    // 4. Deserialize (Deep Copy)
    let deserialized: TestDto = archived.deserialize(&mut rkyv::Infallible).unwrap();
    assert_eq!(original, deserialized);
}

#[test]
fn test_vesicle_preallocation() {
    let mut v = Vesicle::with_capacity(100);
    assert_eq!(v.len(), 100);

    // Write directly to slice
    v.as_mut_slice()[0] = 0xFF;
    v.as_mut_slice()[99] = 0xAA;

    assert_eq!(v.as_slice()[0], 0xFF);
    assert_eq!(v.as_slice()[99], 0xAA);
}