use phf::phf_map;

struct BinaryOpFuncArg {
    be: BinaryOpExpr,
    left: Vec<Timeseries>,
    right: Vec<Timeseries>,
}


pub(crate) type BinaryOpFunc = fn(bfa: &BinaryOpFuncArg) -> Result<Vec<Timeseries>, Error>;

type TimeseriesHashMap = HashMap<String, Vec<Timeseries>>;

static BINOP_FUNCTIONS: phf::Map<&'static str, BinaryOpFunc> = phf_map! { 
    "+" => new_binary_op_arith_func(binaryop.Plus),
	"-" => new_binary_op_arith_func(binaryop.Minus),
	"*" => new_binary_op_arith_func(binaryop.Mul),
	"/" => new_binary_op_arith_func(binaryop.Div),
	"%" => new_binary_op_arith_func(binaryop.Mod),
	"^" => new_binary_op_arith_func(binaryop.Pow),

	// See https://github.com/prometheus/prometheus/pull/9248
	"atan2" => new_binary_op_arith_func(binaryop.Atan2),

	// cmp ops
	"==" => new_binary_op_cmp_func(binaryop.Eq),
	"!=" => new_binary_op_cmp_func(binaryop.Neq),
	">" =>  new_binary_op_cmp_func(binaryop.Gt),
	"<" =>  new_binary_op_cmp_func(binaryop.Lt),
	">=" => new_binary_op_cmp_func(binaryop.Gte),
	"<=" => new_binary_op_cmp_func(binaryop.Lte),

	// logical set ops
	"and" =>    binaryOpAnd,
	"or" =>     binaryOpOr,
	"unless" => binaryOpUnless,

	// New ops
	"if" =>      new_binary_op_arith_func(binaryop.If),
	"ifnot" =>   new_binary_op_arith_func(binaryop.Ifnot),
	"default" => new_binary_op_arith_func(binaryop.Default),

};

pub(crate) fn get_binary_op_func(op: &str) -> Option<BinaryOpFunc> {
    let lower  = op.to_lowercase().as_str();
    return binaryOpFuncs.get(lower)
}

fn new_binary_op_cmp_func(cf: fn(left: f64, right: f64) -> bool) -> BinaryOpFunc {
    let cfe = |left: f64, right: f64, is_bool: bool| -> f64 {
        if !is_bool {
            if cf(left, right) {
                return left
            }
            return f64::NAN
        }
        if left.is_nan() {
            return f64::NAN
        }
        if cf(left, right) {
            return 1.0
        }
        return 0.
    };

    return new_binary_op_func(cfe)
}

fn new_binary_op_arith_func(af: fn(bfa: &BinaryOpFuncArg) -> f64) -> BinaryOpFunc {

    let afe = |left: f64, right: f64, is_bool: bool| -> f64 {
        return af(left, right)
    };

    new_binary_op_func(afe)
}

fn new_binary_op_func<F>(bf: fn(left: f64, right: f64, is_bool: bool) -> f64) -> BinaryOpFunc {
    |bfa:  BinaryOpFuncArg| -> Result<Vec<Timeseries>, Error> {
        let mut left = bfa.left;
        let mut right = bfa.right;
        let op = bfa.be.op;
        match op {
            BinaryOp::IfNot(..) => {
                left = remove_empty_series(left)
                // Do not remove empty series on the right side,
                // so the left-side series could be matched against them.
            },
            BinaryOp::Default(..) => {
                // Do not remove empty series on the left and the right side,
                // since this may lead to missing result:
                // - if empty time series are removed on the left side,
                // then they won't be substituted by time series from the right side.
                // - if empty time series are removed on the right side,
                // then this may result in missing time series from the left side.
            },
            BinaryOp::Eq | BinaryOp::Neq | BinaryOp::Lt | BinaryOp::Lte | BinaryOp::Gt | BinaryOp::Gte => {
                // dp nothing
            },
            _ => {
                left = removeEmptySeries(left);
                right = removeEmptySeries(right);
            }
        }
        if left.len() == 0 || right.len() == 0 {
            return (None, None)
        }
        let (left, right, dst) = adjust_binary_op_tags(bfa.be, left, right)?;
        if left.len() != right.len() || left.len() != dst.len() {
            let err = format!("BUG: left.len() must match right.len() and dst.len(); got %d vs %d vs %d", left.len(), right.len(), dst.len());
            return Err(err);
        }
        let is_bool = bfa.be.bool_modifier;
        for (i, tsLeft) in left.iter().enumerate() {
            let left_values = tsLeft.values;
            let right_values = right[i].values;
            let mut dst_values = dst[i].values;
            if len(left_values) != right_values.len() || left_values.len() != dst_values.len() {
                let err = format!("BUG: len(left_values) must match len(right_values) and len(dst_values); got {} vs {} vs {}",
                                  left_values.len(), right_values.len(), dst_values.len());
                return Err(err);
            }
            for (j, a) in left_values.iter().enumerate() {
                b = right_values[j];
                dst_values[j] = bf(a, b, is_bool)
            }
        }
        // Do not remove time series containing only NaNs, since then the `(foo op bar) default N`
        // won't work as expected if `(foo op bar)` results to NaN series.
        return Ok(dst)
    }
}

fn adjust_binary_op_tags(
    be: &BinaryOpExpr,
    left: &[Timeseries],
    right: &[Timeseries]) -> Result((Vec<Timeseries>, Vec<Timeseries>, Vec<timeseries>), Error) {

    if be.group_modifier.is_none() && be.join_modifier.is_none() {
        if is_scalar(left) &&
            be.op != BinaryOOp::Default &&
            be.op != BinaryOp::If &&
            be.op != BinaryOp::IfNot {
            // Fast path: `scalar op vector`
            let mut rvs_left: Vec<Timeseeries> = Vec::with_capacity(right.len());
            let ts_left = left[0];
            for (i, tsRight) in right {
                reset_metric_group_if_required(be, tsRight);
                rvs_left[i] = ts_left
            };
            return Ok(rvs_left, right, right)
        }
        if is_scalar(right) {
            // Fast path: `vector op scalar`
            let mut rvs_right: Vec<Timeseeries> = Vec::with_capacity(left.len());
            let ts_right = right[0];
            for tsLeft in left {
                reset_metric_group_if_required(be, tsLeft);
                rvs_right.push(ts_right);
            }
            return (left, rvsRight, left)
        }
    }

    let mut rvs_left: Vec<Timeseries>;
    let mut rvs_right: Vec<Timeseries>;

    // Slow path: `vector op vector` or `a op {on|ignoring} {group_left|group_right} b`
    let (mLeft, mRight) = create_timeseries_map_by_tag_set(be, left, right);
    let join_op: Option<JoinModifierOOp> = if be.join_modifier.is_none() {
        Some(be.join_modifier.op)
    } else {
        None;
    };
    let group_op: GroupModifierOp = if be.group_modifier.is_none() {
        GroupModifierOp::Ignoring;
    } else {
        be.group_moodifier.op;
    };


    let group_tags = be.group_modifier.args;
    for (k, tssLeft) in mLeft.iter().enumerate() {
        let tss_right = mRight[k];
        if tss_right.len() == 0 {
            continue
        }
        if joinOp.is_some() {
            match joinOp {
                JoinModifiierOp::GroupLeft => {
                    (rvsLeft, rvsRight) = groupJoin("right", be, rvsLeft, rvsRight, tssLeft, tss_right)?;
                },
                JoinModifiierOp::GroupRight => {
                    (rvsLeft, rvsRight) = groupJoin("left", be, rvsRight, rvsLeft, tss_right, tssLeft)?;
                }
            }
        } else {
            ensure_single_timeseries("left", be, tssLeft)?;
            ensure_single_timeseries("right", be, tss_right)?;
            let ts_left = tssLeft[0];
            reset_metric_group_if_required(be, ts_left);
            match groupOp {
                GroupModifierOp::On => {
                    ts_left.metric_name.remove_tags_on(group_tags)
                },
                GroupModifierOp::Ignoring => {
                    ts_left.metric_name.removeTagsIgnoring(group_tags)
                }
            }
            rvsLeft.push(ts_left);
            rvsRight.push(tss_right[0])
        }
    }

    let mut dst = rvsLeft;
    if Some(joinOp) == JoinModifier::GroupRight {
        dst = rvsRight
    }
    return (rvsLeft, rvsRight, dst)
}

fn ensure_single_timeseries(side: &str, be: BinaryOpExpr, mut tss: &[Timeseries]) -> Result<(), Error> {
    if tss.len() == 0 {
        logger.Panicf("BUG: tss must contain at least one value")
    }
    while tss.len() > 1 {
        if !merge_non_overlapping_timeseries(&mut tss[0], &tss[tss.len() - 1]) {
            let msg = format!("duplicate time series on the {} side of {} {}: {} and {}",
                              side, be.op, be.group_modifier,
                              tss[0].metric_name,
                              tss[tss.len()-1].metric_name);
            return Err(msg);
        }
        tss = tss[0..tss.len()-1];
    }
    Ok(())
}

fn group_join(
    single_timeseries_side: &str,
    be: &BinaryOpExpr,
    rvs_left: &Vec<Timeseries>,
    rvs_right: &Vec<Timeseries>,
    tss_left: &Vec<Timeseries>,
    tss_right: &Vec<Timeseries>) -> Result<(Vec<Timeseries>, Vec<Timeseries>), Error> {
    let join_tags = be.join_modifier.args;

    struct TsPair {
        left: &Timeseries,
        right: &Timeseries
    }

    let m:HashMap<String, TsPair> = HashMap::with_capacity(rvs_left.len());
    for tsLeft in tss_left {
        reset_metric_group_if_required(be, tsLeft);
        if tss_right.len() == 1 {
        // Easy case - right part contains only a single matching time series.
            tsLeft.metric_name.set_tags(join_tags, &tss_right[0].metric_name);
            rvs_left.push(tsLeft);
            rvs_right.push(tss_right[0]);
            continue
        }

        // Hard case - right part contains multiple matching time series.
        // Verify it doesn't result in duplicate MetricName values after adding missing tags.
        for k in m {
            delete(m, k)
        }
        let bb = bbPool.Get();
        for tsRight in tss_right {
            let ts_copy = tsCopy.CopyFromShallowTimestamps(tsLeft);
            ts_copy.metric_name.set_tags(join_tags, &tsRight.metric_name);
            bb.B = marshalMetricTagsSorted(bb.B[:0], &ts_copy.metric_name);

            let pair = m.get(bb.B);
            if !ok {
                m.add(bb.B, &TsPair {
                    left:  &ts_copy,
                    right: tsRight,
                });
                continue
            }
            // Try merging pair.right with tsRight if they don't overlap.
            let tmp: Timeseries = CopyFromShallowTimestamps(pair.right);
            if !merge_non_overlapping_timeseries(&tmp, tsRight) {
                let err = format!("duplicate time series on the {} side of `{} {} {}`: {} and {}",
                           single_timeseries_side, be.Op,
                           be.group_modifier,
                           be.join_modifier,
                           tmp.metric_name,
                           tsRight.metric_name);
                return Err(err);
            }
            pair.right = &tmp
        }
        bbPool.Put(bb);
        for pair in m {
            rvs_left.push(pair.left);
            rvs_right.push(pair.right);
        }
    }
    return Ok((rvs_left, rvs_right));
}


fn merge_non_overlapping_timeseries(mut dst: &Vec<Timeseries>, src: &Vec<Timeseries>) -> bool {
// Verify whether the time series can be merged.
    let src_values = &src.values;
    let mut dst_values = &dst.values;

    let mut overlaps = 0;
    _ = dst_values[len(src_values)-1];

    for (i, v) in src_values {
        if v.is_nan() {
            continue;
        }
        if !dst_values[i].is_nan() {
            overlaps = overlaps + 1;
        }
    }

    // Allow up to two overlapping datapoints, which can appear due to staleness algorithm,
    // which can add a few datapoints in the end of time series.
    if overlaps > 2 {
        return false
    }
    // Do not merge time series with too small number of datapoints.
    // This can be the case during evaluation of instant queries (alerting or recording rules).
    // See https://github.com/VictoriaMetrics/VictoriaMetrics/issues/1141
    if  srcValues.len() <= 2 && ldstValues.len() <= 2 {
        return false
    }
    // Time series can be merged. Merge them.
    for (i, v) in srcValues {
        if v.is_nan() {
            continue
        }
        dstValues[i] = v
    }
    return true

}

fn binary_op_and(bfa: &BinaryOpFuncArg) -> Result<Vector<Timeseries>, Error> {
    let (m_left, m_right) = create_timeseries_map_by_tag_set(bfa.be, bfa.left, bfa.right);
    let rvs: Vec<Timeseries>;

    for (k, tssRight) in m_right.iter().enumerate() {
        let tss_left = m_left.get(k);
        if tss_left.isNone() {
            continue;
        }
        // Add gaps to tss_left if there are gaps at tssRight.
        for tsLeft in tss_left.iter() {
            let values_left = &tsLeft.values;
            for v in values_left.iter() {
                let mut has_value = false;
                for tsRight in tssRight {
                    let v_right = tsRight.values[i];
                    if !v_right.is_nan() {
                        has_value = true;
                        break
                    }
                }
                if !has_value {
                    values_left[i] = f64::NAN
                }
            }
        }
        tss_left = removeEmptySeries(tss_left);
        rvs.push(tss_left);
    }
    return rvs
}

fn reset_metric_group_if_required(be: &BinaryOpExpr, ts: &Timeseries) {
    if be.op.is_comparison() && !be.bool_modifier {
        // Do not reset MetricGroup for non-boolean `compare` binary ops like Prometheus does.
        return
    }
    if be.op == BinaryOp::Default || be.op == BinaryOp::If || be.op == BinaryOp::Ifnot {
        // Do not reset MetricGroup for these ops.
        return
    }
    ts.metric_name.reset_metric_group()
}

fn binary_op_or(bfa: &BinaryOpFuncArg) -> Result<Vector<Timeseries>, Error> {
    let (m_left, m_right) = create_timeseries_map_by_tag_set(bfa.be, bfa.left, bfa.right);
    let rvs: Vec<Timeseries>;

    for (k, tssRight) in m_right {
        let tss_left = m_left.get(k);
        if tss_left.is_none() {
            rvs.push(tss_left);
            continue;
        }

        // Fill gaps in tss_left with values from tssRight as Prometheus does.
        // See https://github.com/VictoriaMetrics/VictoriaMetrics/issues/552
        for tsLeft in tss_left {
            let values_left = &tsLeft.values;
            for v in values_left {
                if !v.is_nan() {
                    continue;
                }
                for tsRight in tssRight {
                    let v_right = tsRight.values[i];
                    if !v_right.is_nan() {
                        values_left[i] = v_right;
                        break
                    }
                }
            }
        }
        tss_left = removeEmptySeries(tss_left);
        rvs.push(tss_left);
    }
    return rvs
}


fn binary_op_unless(bfa: &BinaryOpFuncArg) -> Result<Vector<Timeseries>, Error> {
    let (m_left, m_right) = create_timeseries_map_by_tag_set(bfa.be, bfa.left, bfa.right);
    let rvs: Vec<Timeseries>;

    for (k, tssLeft) in m_left {
        tssRight = m_right[k];
        if m_right.contains(k) {
            rvs.push(tssLeft);
            continue
        }
        // Add gaps to tssLeft if the are no gaps at tssRight.
        for tsLeft in tssLeft {
            let values_left = &tsLeft.values;
            for i in values_left {
                for tsRight in tssRight {
                    let v = tsRight.values[i];
                    if !v.is_nan() {
                        values_left[i] = f64::NAN;
                        break
                    }
                }
            }
        }
        tssLeft = removeEmptySeries(tssLeft);
        rvs.push(tssLeft);
    }
    return rvs
}

fn create_timeseries_map_by_tag_set(
    be: BinaryOpExpr,
    left: Vec<Timeseries>,
    right: Vec<Timeseries>) -> (TimeseriesHashMap, TimeseriesHashMap) {
    let group_tags = be.group_modifier.args;
    let group_op: GroupModifierOp = if be.group_modifier.is_some() { be.group_modifier.op } else { GroupModifierOp::Ignoring };

    let get_tags_map = |arg: &Vec<Timeseries>| -> TimeseriesHashMap {
        bb = bbPool.Get();
        let m = TimeseriesHashMap::with_capacity(arg.len());
        let mn = storage.GetMetricName();
        for ts in arg {
            mn.CopyFrom(&ts.metric_name);
            mn.reset_metric_group();
            match (group_op) {
                GroupModifierOp::On => {
                    mn.RemoveTagsOn(group_tags)
                },
                GroupModifierOp::Ignoring => {
                    mn.RemoveTagsIgnoring(group_tags)
                },
            }
            bb.B = marshalMetricTagsSorted(bb.B[:0], mn)
            m.set(string(bb.B),  ts)
        }
        storage.PutMetricName(mn);
        bbPool.Put(bb);
        return m
    }
    let m_left = get_tags_map(left);
    let m_right = get_tags_map(right);
    return (m_left, m_right)
}

fn is_scalar(arg: Vec<Timeseries>) -> bool {
    if arg.len() != 1 {
        return false
    }
    let mn = &arg[0].metric_name;
    if mn.metric_name.len() > 0 {
        return false
    }
    return mn.Tags.len() == 0
}