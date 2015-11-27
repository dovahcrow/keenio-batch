use protocol::*;
use error::NativeResult;
use keen::*;
use std::time::Duration;
use serde_json::from_reader;
use serde_json::from_str;
use serde_json::to_string;
use serde::Deserialize;
use serde::Serialize;
use cache::open_redis;
use std::convert::Into;
use chrono::UTC;
use error::NativeError;
use hyper::status::StatusCode;
use redis::Connection;
use redis::Commands;
use std::io::Read;

macro_rules! timeit {
    ($e: expr, $f: expr, $t: expr) => {
        {
            let t = UTC::now();
            let result = $e;
            if $t { info!("keen native: {} :{}", $f, UTC::now() - t) }
            result
        }
    };
    ($e: expr, $f: expr) => {
        {
            let t = UTC::now();
            let result = $e;
            info!("{} :{}", $f, UTC::now() - t);
            result
        }
    };
}

pub struct KeenCacheClient {
    client: KeenClient,
    redis: Option<Connection>
}

impl KeenCacheClient {
    pub fn new(key: &str, project: &str) -> KeenCacheClient {
        KeenCacheClient {
            client: KeenClient::new(key, project),
            redis: None
        }
    }
    pub fn set_redis(&mut self, url: &str) -> NativeResult<()> {
        let client = try!(open_redis(url));
        self.redis = Some(client);
        Ok(())
    }
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.client.timeout(timeout);
    }
    pub fn query(&self, metric: Metric, collection: String, timeframe: TimeFrame) -> KeenCacheQuery {
        KeenCacheQuery {
            query: self.client.query(metric, collection, timeframe),
            redis: self.redis.as_ref()
        }
    }
}

pub struct KeenCacheQuery<'a> {
    query: KeenQuery<'a>,
    redis: Option<&'a Connection>
}

impl<'a> KeenCacheQuery<'a> {
    pub fn group_by(&mut self, g: &str) {
        self.query.group_by(g);
    }
    pub fn filter(&mut self, f: Filter) {
        self.query.filter(f);
    }
    pub fn interval(&mut self, i: Interval) {
        self.query.interval(i);
    }
    pub fn max_age(&mut self, age: usize) {
        self.query.max_age(age);
    }
    pub fn data<C>(&self) -> NativeResult<KeenCacheResult<C>> where C: Deserialize {
        let mut resp = try!(timeit!(self.query.data(), "get data from keen io"));
        if resp.status != StatusCode::Ok {
            let e: KeenError = try!(from_reader(resp));
            return Err(NativeError::KeenError(e));
        }
        let mut s = String::new();
        let _ = resp.read_to_string(&mut s);
        debug!("recv data: {}", s);
        let ret = KeenCacheResult {
            data: try!(from_str(&s)),
            redis: self.redis.clone(),
            type_tag: ResultType::POD
        };
        Ok(ret)
    }
}

#[repr(C)]
#[derive(Clone,Copy)]
pub enum ResultType {
    POD = 0,
    ITEMS = 1,
    DAYSPOD = 2,
    DAYSITEMS = 3,
}

pub struct KeenCacheResult<'a, C> {
    pub type_tag: ResultType, // this is for ffi use, so it will be set in ffi module
    data: KeenResult<C>,
    redis: Option<&'a Connection>
}

impl<'a> KeenCacheResult<'a, i64> { pub fn tt(&mut self) { self.type_tag = ResultType::POD } }
impl<'a> KeenCacheResult<'a, Items> { pub fn tt(&mut self) { self.type_tag = ResultType::ITEMS } }
impl<'a> KeenCacheResult<'a, Days<i64>> { pub fn tt(&mut self) { self.type_tag = ResultType::DAYSPOD } }
impl<'a> KeenCacheResult<'a, Days<Items>> { pub fn tt(&mut self) { self.type_tag = ResultType::DAYSITEMS } }

impl<'a,C> KeenCacheResult<'a, C> where C: Deserialize {
    pub fn from_redis(url: &str, key: &str) -> NativeResult<KeenCacheResult<'a,C>> {
        let c = try!(open_redis(url));
        let s: String = try!(c.get(key));
        let result = try!(from_str(&s));
        Ok(KeenCacheResult {
            type_tag: ResultType::POD,
            data: result,
            redis: None
        })
    }
}

impl<'a, C> KeenCacheResult<'a, C> where C: Serialize {
    pub fn accumulate<O>(self) -> KeenCacheResult<'a, O> where KeenResult<C>: Accumulate<O>{
        let r = KeenCacheResult {
            data: self.data.accumulate(),
            redis: self.redis,
            type_tag: ResultType::POD
        };
        r
    }
    pub fn select<O>(self, predicate: (&str, StringOrI64)) -> KeenCacheResult<'a, O> where KeenResult<C>: Select<O> {
        let r = KeenCacheResult {
            data: self.data.select(predicate),
            redis: self.redis,
            type_tag: ResultType::POD
        };
        r
    }
    pub fn to_redis(&self, key: &str, expire: u64) -> NativeResult<()> {
        let bin = try!(to_string(&self.data));
        if self.redis.is_some() {
            let _ = try!(self.redis.unwrap().set(&key[..], bin));
            let _ = try!(self.redis.unwrap().expire(&key[..], expire as usize));
        }
        Ok(())
    }
}

impl<'a, C> Into<String> for KeenCacheResult<'a, C> where C: Serialize {
    fn into(self) -> String {
        to_string(&self.data).unwrap()
    }
}