use redis_module::native_types::RedisType;
use redis_module::{raw, redis_module, Context, RedisResult, NextArg, RedisString, RedisError};
use tdigest_rs::TDigest;
use std::os::raw::{c_void};
use redis_module::RedisModuleIO;
use redis_module::save_double;
use redis_module::save_unsigned;
use tdigest_rs::Centroid;

struct MyType {
    data: TDigest,
}

static MY_REDIS_TYPE: RedisType = RedisType::new(
    "t_digest1",
    0,
    raw::RedisModuleTypeMethods {
        version: raw::REDISMODULE_TYPE_METHOD_VERSION as u64,
        rdb_load: Some(rdb_load),
        rdb_save: Some(rdb_save),
        aof_rewrite: None,
        free: Some(free),

        // Currently unused by Redis
        mem_usage: None,
        digest: None,

        // Aux data
        aux_load: None,
        aux_save: None,
        aux_save_triggers: 0,

        free_effort: None,
        unlink: None,
        copy: None,
        defrag: None,

        copy2: None,
        free_effort2: None,
        mem_usage2: None,
        unlink2: None,
    },
);

unsafe extern "C" fn rdb_save(rdb: *mut RedisModuleIO, value: *mut c_void) {
    let tdigest: &TDigest = &*(value.cast::<TDigest>());
    // Save the simple fields
    save_unsigned(rdb, tdigest.max_size() as u64);
    save_double(rdb, tdigest.min());
    save_double(rdb, tdigest.max());
    save_double(rdb, tdigest.sum());
    save_double(rdb, tdigest.count());
    for centroid in tdigest.centroids() {
        save_double(rdb, centroid.mean());
        save_double(rdb, centroid.weight());
    }
}

unsafe extern "C" fn rdb_load(rdb: *mut RedisModuleIO, encver: i32) -> *mut c_void {
    if encver != 0 {
        // Handle unexpected encoding version
        return std::ptr::null_mut();
    }
    // Load the simple fields
    let max_size = redis_module::load_unsigned(rdb).unwrap() as usize; // if this returns a usize
    let min = redis_module::load_double(rdb).unwrap();
    let max = redis_module::load_double(rdb).unwrap();
    let sum = redis_module::load_double(rdb).unwrap();
    let count = redis_module::load_double(rdb).unwrap() as usize; // if you need it as usize

    let mut centroids = Vec::new();
    for _ in 0..count {
        let mean = redis_module::load_double(rdb).unwrap();
        let weight = redis_module::load_double(rdb).unwrap();
        centroids.push(Centroid::new(mean, weight));
    }
    // let sz = centroids.len();
    let data = TDigest::new(centroids, sum, count as f64, max, min, max_size); // if max_size needs to be usize
    Box::into_raw(Box::new(data)) as *mut c_void
}

unsafe extern "C" fn free(value: *mut c_void) {
    drop(Box::from_raw(value.cast::<MyType>()));
}

enum MergeType {
    SORTED,
    UNSORTED,
}

fn merge(ctx: &Context, args: Vec<RedisString>, merge: MergeType, size: usize) -> RedisResult {
    if args.len() < 2 {
        return Err(RedisError::WrongArity);
    }

    let key = args.clone().into_iter().skip(1).next_arg()?;

    let nums = args
        .into_iter()
        .skip(2)
        .map(|s| (&s).parse_float())
        .collect::<Result<Vec<f64>, RedisError>>()?;

    let len = nums.len();
    let key = ctx.open_key_writable(&key);

    match key.get_value::<MyType>(&MY_REDIS_TYPE)? {
        Some(value) => {
            let new_data = value.data.merge_unsorted(nums);
            value.data = new_data
        }
        None => {
            let mut data = TDigest::new_with_size(size);
            match merge {
                MergeType::SORTED => data = data.merge_sorted(nums),
                MergeType::UNSORTED => data = data.merge_unsorted(nums),
            }
            key.set_value(&MY_REDIS_TYPE, MyType { data })?;
        }
    }
    ctx.replicate_verbatim();
    ctx.reply_simple_string("OK");
    Ok(len.into())
}

fn alloc_merge_unsorted(ctx: &Context, args: Vec<RedisString>) -> RedisResult {
    merge(ctx, args, MergeType::UNSORTED, 100)
}

fn alloc_merge_sorted(ctx: &Context, args: Vec<RedisString>) -> RedisResult {
    merge(ctx, args, MergeType::SORTED, 100)
}

fn alloc_get(ctx: &Context, args: Vec<RedisString>) -> RedisResult {
    if args.len() < 2 {
        return Err(RedisError::WrongArity);
    }

    let mut args = args.into_iter().skip(1);
    let key = args.next_arg()?;

    let percentile = args.next_f64()?;

    let key = ctx.open_key(&key);

    let value = match key.get_value::<MyType>(&MY_REDIS_TYPE)? {
        Some(value) => value.data.estimate_quantile(percentile).into(),
        None => ().into(),
    };

    Ok(value)
}

//////////////////////////////////////////////////////

redis_module! {
    name: "percentile",
    version: 1,
    allocator: (redis_module::alloc::RedisAlloc, redis_module::alloc::RedisAlloc),
    data_types: [
        MY_REDIS_TYPE,
    ],
    commands: [
        ["percentile.merge", alloc_merge_unsorted, "write", 1, 1, 1],
        ["percentile.mergesorted", alloc_merge_sorted, "write", 1, 1, 1],
        ["percentile.get", alloc_get, "readonly", 1, 1, 1],
    ],
}
