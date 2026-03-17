use std::net::IpAddr;

use crate::ip::IpDetail;

pub fn render_node_name(pattern: &str, proxy_ip: &IpAddr, ip_detail: &IpDetail) -> String {
    let cn_region = coarse_cn_region(ip_detail);

    pattern
        .replace("${CN_COUNTRY}", cn_region)
        .replace("${COUNTRYCODE}", &ip_detail.country_code)
        .replace("${CN_REGION}", cn_region)
        .replace("${COUNTRY}", &ip_detail.country)
        .replace("${REGION}", &ip_detail.region)
        .replace("${CITY}", &ip_detail.city)
        .replace("${ISP}", &ip_detail.isp)
        .replace("${IP}", &proxy_ip.to_string())
}

pub fn coarse_cn_region(ip_detail: &IpDetail) -> &'static str {
    let country_code = ip_detail.country_code.trim().to_ascii_uppercase();
    match country_code.as_str() {
        "CN" => "中国",
        "HK" => "香港",
        "TW" => "台湾",
        "MO" => "澳门",
        "JP" => "日本",
        "KR" => "韩国",
        "SG" => "新加坡",
        "US" => "美国",
        "GB" => "英国",
        "DE" => "德国",
        "FR" => "法国",
        "NL" => "荷兰",
        "CA" => "加拿大",
        "AU" => "澳大利亚",
        "MY" => "马来西亚",
        "TH" => "泰国",
        "IN" => "印度",
        "ID" => "印度尼西亚",
        "VN" => "越南",
        "PH" => "菲律宾",
        "AE" => "阿联酋",
        "TR" => "土耳其",
        "RU" => "俄罗斯",
        _ => infer_cn_region_from_text(ip_detail),
    }
}

fn infer_cn_region_from_text(ip_detail: &IpDetail) -> &'static str {
    let geo_text = format!(
        "{} {} {} {}",
        ip_detail.country, ip_detail.region, ip_detail.city, ip_detail.timezone
    )
    .to_lowercase();

    if contains_any(&geo_text, &["hong kong", "香港"]) {
        "香港"
    } else if contains_any(&geo_text, &["taiwan", "台灣", "台湾"]) {
        "台湾"
    } else if contains_any(&geo_text, &["macau", "macao", "澳门"]) {
        "澳门"
    } else if contains_any(&geo_text, &["japan", "日本"]) {
        "日本"
    } else if contains_any(&geo_text, &["korea", "韩国", "南韩"]) {
        "韩国"
    } else if contains_any(&geo_text, &["singapore", "新加坡"]) {
        "新加坡"
    } else if contains_any(&geo_text, &["united states", "usa", "美国"]) {
        "美国"
    } else if contains_any(&geo_text, &["united kingdom", "britain", "england", "英国"]) {
        "英国"
    } else if contains_any(&geo_text, &["germany", "deutschland", "德国"]) {
        "德国"
    } else if contains_any(&geo_text, &["france", "法国"]) {
        "法国"
    } else if contains_any(&geo_text, &["netherlands", "nederland", "荷兰"]) {
        "荷兰"
    } else if contains_any(&geo_text, &["canada", "加拿大"]) {
        "加拿大"
    } else if contains_any(&geo_text, &["australia", "澳大利亚"]) {
        "澳大利亚"
    } else if contains_any(&geo_text, &["malaysia", "马来西亚"]) {
        "马来西亚"
    } else if contains_any(&geo_text, &["thailand", "泰国"]) {
        "泰国"
    } else if contains_any(&geo_text, &["india", "印度"]) {
        "印度"
    } else if contains_any(&geo_text, &["indonesia", "印尼", "印度尼西亚"]) {
        "印度尼西亚"
    } else if contains_any(&geo_text, &["vietnam", "越南"]) {
        "越南"
    } else if contains_any(&geo_text, &["philippines", "菲律宾"]) {
        "菲律宾"
    } else if contains_any(&geo_text, &["united arab emirates", "uae", "阿联酋"]) {
        "阿联酋"
    } else if contains_any(&geo_text, &["turkey", "turkiye", "土耳其"]) {
        "土耳其"
    } else if contains_any(&geo_text, &["russia", "俄罗斯"]) {
        "俄罗斯"
    } else if contains_any(&geo_text, &["china", "中国"]) {
        "中国"
    } else {
        "其他地区"
    }
}

fn contains_any(content: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| content.contains(needle))
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;

    fn sample_ip_detail(country: &str, country_code: &str, city: &str) -> IpDetail {
        IpDetail {
            ip: "1.1.1.1".to_string(),
            country: country.to_string(),
            country_code: country_code.to_string(),
            isp: "Cloudflare".to_string(),
            city: city.to_string(),
            region: city.to_string(),
            region_code: city.to_string(),
            timezone: "Asia/Hong_Kong".to_string(),
        }
    }

    #[test]
    fn test_render_node_name_with_cn_region() {
        let ip = IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1));
        let ip_detail = sample_ip_detail("Hong Kong", "HK", "Hong Kong");
        let name = render_node_name("${CN_REGION}_${CITY}_${ISP}_${IP}", &ip, &ip_detail);
        assert_eq!(name, "香港_Hong Kong_Cloudflare_1.1.1.1");
    }

    #[test]
    fn test_unknown_region_falls_back_to_other() {
        let ip_detail = sample_ip_detail("Brazil", "BR", "Sao Paulo");
        assert_eq!(coarse_cn_region(&ip_detail), "其他地区");
    }

    #[test]
    fn test_country_text_can_infer_without_country_code() {
        let ip_detail = sample_ip_detail("Singapore", "", "Singapore");
        assert_eq!(coarse_cn_region(&ip_detail), "新加坡");
    }
}
